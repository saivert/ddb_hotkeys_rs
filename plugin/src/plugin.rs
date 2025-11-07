use crate::*;
use ashpd::desktop::{
    global_shortcuts::{
        Activated, Deactivated, GlobalShortcuts, NewShortcut, Shortcut, ShortcutsChanged,
    },
    ResponseError, Session,
};
use async_lock::Mutex;
use futures_util::{
    future::{AbortHandle, Abortable},
    stream::{select_all, Stream, StreamExt},
};
use smol::block_on;

use std::{collections::HashSet, str::FromStr, sync::Arc, thread};

pub struct MiscPlugin {
    plugin: DB_hotkeys_plugin_t,

    thread: Option<PluginThread>,

    shortcut_handler: Arc<Mutex<ShortcutHandler>>,
}

unsafe impl Send for MiscPlugin {}

struct PluginThread {
    handle: thread::JoinHandle<()>,
    sender: smol::channel::Sender<ThreadMessage>,
}

#[derive(Debug)]
enum ThreadMessage {
    Start,
    Terminate,
}

impl PluginThread {
    pub fn new(plugin: Arc<Mutex<ShortcutHandler>>) -> Self {
        let (sender, receiver) = smol::channel::unbounded::<ThreadMessage>();
        Self {
            handle: thread::spawn(move || thread_main(receiver, plugin)),
            sender,
        }
    }

    pub fn join(self) -> thread::Result<()> {
        drop(self.sender); // Close the channel
        self.handle.join()
    }

    pub fn msg(&self, msg: ThreadMessage) {
        self.sender
            .send_blocking(msg)
            .expect("Unable to send message to thread!");
    }
}

impl DBPlugin for MiscPlugin {
    fn get_plugin_ptr(&self) -> *const DB_plugin_t {
        &self.plugin as *const DB_hotkeys_plugin_t as *const DB_plugin_t
    }
}

#[derive(Debug, Clone)]
pub struct RegisteredShortcut {
    id: String,
    activation: String,
}

#[derive(Debug)]
enum Event {
    Activated(Activated),
    Deactivated(Deactivated),
    ShortcutsChanged(ShortcutsChanged),
}

impl MiscPlugin {
    pub fn new(plugin: DB_hotkeys_plugin_t) -> Self {
        Self {
            plugin,
            thread: None,
            shortcut_handler: Arc::new(Mutex::new(ShortcutHandler::new())),
        }
    }

    pub fn plugin_start(&mut self) {
        tracing::info!("[Global Shortcuts] plugin start");
        self.thread = Some(PluginThread::new(self.shortcut_handler.clone()));
        if let Some(s) = self.thread.as_ref() {
            s.msg(ThreadMessage::Start);
        }
    }

    pub fn plugin_stop(&mut self) {
        if let Some(s) = self.thread.as_ref() {
            tracing::info!("[Global Shortcuts] Sending Terminate to thread.");
            s.msg(ThreadMessage::Terminate);
        }

        if let Some(t) = self.thread.take() {
            match t.join() {
                Ok(_) => (),
                Err(_) => {
                    DeadBeef::log_detailed(
                        DDB_LOG_LAYER_INFO,
                        "[Global Shortcuts] Playback thread lingering!\n",
                    );
                }
            }
        }
    }

    #[allow(unused)]
    pub fn message(&self, msgid: u32, ctx: usize, p1: u32, p2: u32) {
        match msgid {
            _ => {}
        }
    }
}

struct ShortcutHandler {
    pub rebind_count: Arc<Mutex<u32>>,
    pub session: Arc<Mutex<Option<Session<'static, GlobalShortcuts<'static>>>>>,
    pub abort_handle: Arc<Mutex<Option<AbortHandle>>>,
    pub triggers: Arc<Mutex<Vec<RegisteredShortcut>>>,
    pub activations: Arc<Mutex<HashSet<String>>>,
}

impl ShortcutHandler {
    pub fn new() -> Self {
        Self {
            rebind_count: Default::default(),
            session: Default::default(),
            abort_handle: Default::default(),
            triggers: Default::default(),
            activations: Default::default(),
        }
    }

    async fn start_session(&self) -> ashpd::Result<()> {
        //let hotkeysconfig = DeadBeef::conf_get_str("hotkey", "");
        let shortcuts: Option<Vec<_>> = Some(vec![
            // Example shortcut
            NewShortcut::new("playpause", "Play/Pause").preferred_trigger("Ctrl+Alt+H"),
            NewShortcut::new("next", "Next song").preferred_trigger("Ctrl+Alt+H"),
            NewShortcut::new("prev", "Previous song").preferred_trigger("Ctrl+Alt+H"),
        ]);

        // Set Application id
        let appid = ashpd::AppID::from_str(&"music.deadbeef.player")?;
        ashpd::register_host_app(appid).await?;

        match shortcuts {
            Some(shortcuts) => {
                let global_shortcuts = GlobalShortcuts::new().await?;
                let session = global_shortcuts.create_session().await?;
                let request = global_shortcuts
                    .bind_shortcuts(&session, &shortcuts[..], None)
                    .await?;
                let response = request.response();
                if let Err(e) = &response {
                    match e {
                        ashpd::Error::Response(ResponseError::Cancelled) => {
                            tracing::error!("[Global Shortcuts] Cancelled\n");
                        }
                        ashpd::Error::Response(ResponseError::Other) => {
                            tracing::error!("[Global Shortcuts] Other response error\n");
                        }
                        other => tracing::error!("{}", other),
                    }
                };

                match response {
                    Ok(resp) => {
                        let triggers: Vec<_> = resp
                            .shortcuts()
                            .iter()
                            .map(|s: &Shortcut| RegisteredShortcut {
                                id: s.id().to_owned(),
                                activation: s.trigger_description().to_owned(),
                            })
                            .collect();
                        *self.triggers.lock().await = triggers;
                        self.display_activations().await;
                        self.set_rebind_count(Some(0));
                        self.session.lock().await.replace(session);
                        loop {
                            if self.session.lock().await.is_none() {
                                break;
                            }

                            let (abort_handle, abort_registration) = AbortHandle::new_pair();
                            let global_shortcuts_arc = Arc::new(&global_shortcuts);
                            let global_shortcuts_clone = global_shortcuts_arc.clone();
                            let future = Abortable::new(
                                self.track_incoming_events(&global_shortcuts_clone),
                                abort_registration,
                            );
                            self.abort_handle.lock().await.replace(abort_handle);
                            tracing::info!("[Global Shortcuts] Awaiting track_incoming_events");
                            let _ = future.await;
                        }
                    }
                    Err(e) => {
                        tracing::error!("[Global Shortcuts] Failure {:?}\n", e);
                    }
                }
            }
            _ => {
                tracing::error!("[Global Shortcuts] Shortcut list invalid\n");
            }
        };
        tracing::info!("[Global Shortcuts] End of start session");
        Ok(())
    }

    fn set_rebind_count(&self, count: Option<u32>) {
        match count {
            None => {}
            Some(count) => DeadBeef::log_detailed(DDB_LOG_LAYER_INFO, &format!("{}", count)),
        }
    }

    async fn track_incoming_events(&self, global_shortcuts: &GlobalShortcuts<'_>) {
        let Ok(activated_stream) = global_shortcuts.receive_activated().await else {
            return;
        };
        let Ok(deactivated_stream) = global_shortcuts.receive_deactivated().await else {
            return;
        };
        let Ok(changed_stream) = global_shortcuts.receive_shortcuts_changed().await else {
            return;
        };

        let bact: Box<dyn Stream<Item = Event> + Unpin + Send> =
            Box::new(activated_stream.map(Event::Activated));
        let bdeact: Box<dyn Stream<Item = Event> + Unpin> =
            Box::new(deactivated_stream.map(Event::Deactivated));
        let bchg: Box<dyn Stream<Item = Event> + Unpin> =
            Box::new(changed_stream.map(Event::ShortcutsChanged));

        let mut events = select_all([bact, bdeact, bchg]);

        tracing::info!("[Global Shortcuts] Starting to wait for events");

        while let Some(event) = events.next().await {
            tracing::info!("[Global Shortcuts] Got new event from stream");
            match event {
                Event::Activated(activation) => {
                    self.on_activated(activation).await;
                }
                Event::Deactivated(deactivation) => {
                    self.on_deactivated(deactivation).await;
                }
                Event::ShortcutsChanged(change) => {
                    self.on_changed(change).await;
                }
            }
        }
    }

    async fn stop(&self) {
        if let Some(abort_handle) = self.abort_handle.lock().await.take() {
            tracing::info!("[Global Shortcuts] Aborting");
            abort_handle.abort();
        }

        if let Some(session) = self.session.lock().await.take() {
            let _ = session.close().await;
        }
        self.set_rebind_count(None);
        self.activations.lock().await.clear();
        self.triggers.lock().await.clear();
    }

    async fn display_activations(&self) {
        let activations = self.activations.lock().await.clone();
        let triggers = self.triggers.lock().await.clone();
        let text: Vec<String> = triggers
            .into_iter()
            .map(|RegisteredShortcut { id, activation }| {
                let escape = |s: &str| s.to_string(); // noop for now
                let id = escape(&id);
                let activation = escape(&activation);
                if activations.contains(&id) {
                    format!("<b>{}: {}</b>", id, activation)
                } else {
                    format!("{}: {}", id, activation)
                }
            })
            .collect();
        tracing::info!("Active Shortcuts:\n{}\n", text.join("\n"));
    }

    async fn on_activated(&self, activation: Activated) {
        {
            let mut activations = self.activations.lock().await;
            activations.insert(activation.shortcut_id().into());

            match activation.shortcut_id() {
                "playpause" => DeadBeef::call_action_by_name("play_pause"),
                "next" => DeadBeef::call_action_by_name("next"),
                "prev" => DeadBeef::call_action_by_name("prev"),
                _ => {}
            }
        }

        self.display_activations().await
    }

    async fn on_deactivated(&self, deactivation: Deactivated) {
        {
            let mut activations = self.activations.lock().await;
            if !activations.remove(deactivation.shortcut_id()) {
                tracing::error!(
                    "Received deactivation without previous activation: {deactivation:?}"
                );
            }
        }
        self.display_activations().await
    }

    async fn on_changed(&self, change: ShortcutsChanged) {
        *self.triggers.lock().await = change
            .shortcuts()
            .iter()
            .map(|s| RegisteredShortcut {
                id: s.id().to_owned(),
                activation: s.trigger_description().to_owned(),
            })
            .collect();
        *self.rebind_count.lock().await += 1;
        self.set_rebind_count(Some(*self.rebind_count.lock().await));
        self.display_activations().await
    }
}

fn thread_main(
    receiver: smol::channel::Receiver<ThreadMessage>,
    plugin: Arc<Mutex<ShortcutHandler>>,
) {
    block_on(async {
        while let Ok(msg) = receiver.recv().await {
            match msg {
                ThreadMessage::Terminate => {
                    tracing::info!("[Global Shortcuts] Plugin thread terminating...");
                    plugin.lock().await.stop().await;
                }
                ThreadMessage::Start => {
                    tracing::info!("[Global Shortcuts] Plugin thread received Start message");
                    if plugin.lock().await.start_session().await.is_ok() {
                        tracing::info!("[Global Shortcuts] Plugin session started successfully");
                    } else {
                        tracing::info!("[Global Shortcuts] Plugin session failed to start");
                    }
                }
            }
        }
    });
}
