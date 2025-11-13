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
    stream::{select_all, AbortRegistration, Stream, StreamExt},
};
use smol::channel;
use std::{collections::HashSet, str::FromStr, sync::Arc, thread};

use keysyms::parse_shortcut;

pub struct MiscPlugin {
    plugin: DB_hotkeys_plugin_t,
    thread: Option<PluginThread>,
    shortcut_handler: Arc<Mutex<ShortcutHandler>>,
    abort_handle: Arc<Mutex<Option<AbortHandle>>>,
    commands: Vec<Command>,
}

#[derive(Debug, Clone, Copy)]
pub struct Command {
    keycode: i32,
    modifier: i32,
    ctx: ddb_action_context_t,
    isglobal: i32,
    action: *mut DB_plugin_action_t,
}

unsafe impl Send for MiscPlugin {}

struct PluginThread {
    handle: thread::JoinHandle<()>,
    sender: channel::Sender<ThreadMessage>,
}

#[derive(Debug)]
enum ThreadMessage {
    Start,
    Terminate,
}

impl PluginThread {
    pub fn new(plugin: Arc<Mutex<ShortcutHandler>>) -> Self {
        let (sender, receiver) = channel::bounded::<ThreadMessage>(10);
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
            .try_send(msg)
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
        let (abort_handle, abort_registration) = AbortHandle::new_pair();

        Self {
            plugin,
            thread: None,
            shortcut_handler: Arc::new(Mutex::new(ShortcutHandler::new(abort_registration))),
            abort_handle: Arc::new(Mutex::new(Some(abort_handle))),
            commands: Vec::new(),
        }
    }

    pub fn plugin_start(&mut self) {
        tracing::debug!("plugin start");

        self.read_config();

        self.thread = Some(PluginThread::new(self.shortcut_handler.clone()));
        if let Some(s) = self.thread.as_ref() {
            s.msg(ThreadMessage::Start);
        }
    }

    pub fn plugin_stop(&mut self) {
        self.abort_handle
            .lock_blocking()
            .take()
            .expect("Abort handle")
            .abort();

        if let Some(s) = self.thread.as_ref() {
            tracing::debug!("Sending Terminate to thread.");
            s.msg(ThreadMessage::Terminate);
        }

        if let Some(t) = self.thread.take() {
            match t.join() {
                Ok(_) => (),
                Err(_) => {
                    DeadBeef::log_detailed(DDB_LOG_LAYER_INFO, "Playback thread lingering!\n");
                }
            }
        }
    }

    fn read_config(&mut self) {
        for a in DeadBeef::conf_find_str("hotkey.").into_iter().flatten() {
            if let Some(value) = a.value() {
                match parse_line(value) {
                    Ok((keystroke, isglobal, action_name, ctx)) => {
                        tracing::debug!("keystroke: {keystroke}, isglobal: {isglobal}, action_name: {action_name}, ctx: {ctx}");
                        if let Some((keycode, modifier)) = parse_shortcut(&keystroke) {
                            let action = DeadBeef::find_action_by_name(&action_name);
    
                            let new_command = Command {
                                keycode,
                                modifier,
                                ctx,
                                isglobal: isglobal as i32,
                                action: action.map(|x| x.as_ptr()).unwrap_or(std::ptr::null_mut()),
                            };
                            tracing::debug!("new_command: {new_command:?}");
                            self.commands.push(new_command);
                        }
                    }
                    Err(msg) => tracing::error!("Unable to parse hotkey config item: {msg}"),
                }
            }
        }
    }

    pub fn get_action_for_keycombo(&mut self,
        key: i32,
        mods: i32,
        isglobal: i32
    ) -> Option<(ddb_action_context_t, *mut DB_plugin_action_t)> {
        let act = self.commands.iter().find(|x | {
            x.isglobal == isglobal && x.keycode == key && x.modifier == mods
        });

        act.map(|x| (x.ctx, x.action))
    }

    // #[allow(unused)]
    // pub fn message(&self, msgid: u32, ctx: usize, p1: u32, p2: u32) {
    //     match msgid {
    //         _ => {}
    //     }
    // }
}

struct ShortcutHandler {
    pub session: Arc<Mutex<Option<Session<'static, GlobalShortcuts<'static>>>>>,
    // pub abort_handle: Arc<Mutex<Option<AbortHandle>>>,
    pub abort_registration: std::cell::Cell<Option<AbortRegistration>>,
    pub triggers: Arc<Mutex<Vec<RegisteredShortcut>>>,
    pub activations: Arc<Mutex<HashSet<String>>>,
}

impl ShortcutHandler {
    pub fn new(abort_registration: AbortRegistration) -> Self {
        Self {
            session: Default::default(),
            abort_registration: std::cell::Cell::new(Some(abort_registration)),
            triggers: Default::default(),
            activations: Default::default(),
        }
    }

    async fn start_session(&self) -> ashpd::Result<()> {
        // Collect shortcuts from configuration entries `hotkey.*`.
        // Each value should parse as: `"<keystroke>" <num1> <num2> <action name...>`
        let mut collected: Vec<_> = Vec::new();
        for a in DeadBeef::conf_find_str("hotkey.").into_iter().flatten() {
            if let Some(value) = a.value() {
                match parse_line(value) {
                    Ok((keystroke, global, action_name, _)) => {
                        if !global {
                            // skip non-global bindings for portal registration
                            continue;
                        }

                        // Use the action title if available, otherwise fall back to the action name

                        let raw_title = DeadBeef::find_action_by_name(&action_name)
                            .and_then(|act| act.title().map(|s| s.to_string()))
                            .unwrap_or_else(|| action_name.clone());

                        let title_segment = last_segment_after_unescaped_slash(&raw_title);

                        // Convert escaped forward slashes ("\/" -> "/") in the final segment
                        let title = title_segment.replace("\\/", "/");

                        tracing::debug!("{keystroke} = {}", title);

                        collected.push(
                            NewShortcut::new(action_name.as_str(), title.as_str())
                                .preferred_trigger(keystroke.as_str()),
                        );
                    }
                    Err(msg) => tracing::error!("Unable to parse hotkey config item: {msg}"),
                }
            }
        }

        // Use only collected shortcuts from config; if none, don't register any shortcuts
        let shortcuts: Option<Vec<_>> = if collected.is_empty() {
            None
        } else {
            Some(collected)
        };

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
                            tracing::error!("Cancelled\n");
                        }
                        ashpd::Error::Response(ResponseError::Other) => {
                            tracing::error!("Other response error\n");
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
                        self.session.lock().await.replace(session);
                        loop {
                            if self.session.lock().await.is_none() {
                                break;
                            }

                            if let Some(ar) = self.abort_registration.take() {
                                let future = Abortable::new(
                                    self.track_incoming_events(&global_shortcuts),
                                    ar,
                                );
                                //self.abort_handle.lock().await.replace(abort_handle);
                                tracing::debug!("Awaiting track_incoming_events");
                                let _ = future.await;
                            } else {
                                break;
                            }
                        }
                    }
                    Err(e) => {
                        tracing::error!("Failure {:?}\n", e);
                    }
                }
            }
            _ => {
                tracing::error!("Shortcut list invalid\n");
            }
        };
        tracing::debug!("End of start session");
        Ok(())
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

        tracing::debug!("Starting to wait for events");

        while let Some(event) = events.next().await {
            tracing::debug!("Got new event from stream");
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
        tracing::debug!("Aborting");

        // if let Some(abort_handle) = self.abort_handle.lock().await.take() {
        //     tracing::debug!("Aborting");
        //     abort_handle.abort();
        // }

        if let Some(session) = self.session.lock().await.take() {
            let _ = session.close().await;
        }
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
        tracing::debug!("Active Shortcuts:\n{}\n", text.join("\n"));
    }

    async fn on_activated(&self, activation: Activated) {
        {
            let mut activations = self.activations.lock().await;
            activations.insert(activation.shortcut_id().into());
            DeadBeef::call_action_by_name(activation.shortcut_id());
        }

        self.display_activations().await
    }

    async fn on_deactivated(&self, deactivation: Deactivated) {
        {
            let mut activations = self.activations.lock().await;
            if !activations.remove(deactivation.shortcut_id()) {
                tracing::debug!(
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

        self.display_activations().await
    }
}

fn thread_main(receiver: channel::Receiver<ThreadMessage>, plugin: Arc<Mutex<ShortcutHandler>>) {
    smol::block_on(async {
        while let Ok(msg) = receiver.recv().await {
            match msg {
                ThreadMessage::Terminate => {
                    tracing::debug!("Plugin thread terminating...");
                    plugin.lock().await.stop().await;
                }
                ThreadMessage::Start => {
                    tracing::debug!("Plugin thread received Start message");
                    if !plugin.lock().await.start_session().await.is_ok() {
                        tracing::debug!("Plugin session failed to start");
                    }
                }
            }
        }
    });
}

/// Parse lines like: `"Ctrl k" 0 0 toggle_stop_after_album`
///
/// Returns (keystroke, is_global, action_name)
pub fn parse_line(line: &str) -> Result<(String, bool, String, ddb_action_context_t), String> {
    let s = line.trim();
    let rest = s
        .strip_prefix('"')
        .ok_or_else(|| "line must start with a double quote for keystroke".to_string())?;

    let end_quote_idx = rest
        .find('"')
        .ok_or_else(|| "missing closing quote for keystroke".to_string())?;
    let keystroke = &rest[..end_quote_idx];

    let after = rest[end_quote_idx + 1..].trim();
    let mut parts = after.split_whitespace();

    // second number -> is_global
    let num2 = parts
        .next()
        .ok_or_else(|| "missing second number".to_string())?;
    let ctx = match num2.parse::<ddb_action_context_t>() {
        Ok(n) => n,
        Err(_) => return Err("second number is not a valid integer".to_string()),
    };

    // second number -> is_global
    let num2 = parts
        .next()
        .ok_or_else(|| "missing second number".to_string())?;
    let is_global = match num2.parse::<i64>() {
        Ok(n) => n != 0,
        Err(_) => return Err("second number is not a valid integer".to_string()),
    };

    let action_tokens: Vec<&str> = parts.collect();
    if action_tokens.is_empty() {
        return Err("missing action name".to_string());
    }
    let action_name = action_tokens.join(" ");

    Ok((keystroke.to_string(), is_global, action_name, ctx))
}

#[cfg(test)]
mod tests {
    use super::parse_line;

    #[test]
    fn parses_example_not_global() {
        let line = "\"Ctrl k\" 0 0 toggle_stop_after_album";
        let (keystroke, is_global, action, _) = parse_line(line).expect("parse failed");
        assert_eq!(keystroke, "Ctrl k");
        assert_eq!(is_global, false);
        assert_eq!(action, "toggle_stop_after_album");
    }

    #[test]
    fn parses_example_global_and_action_with_spaces() {
        let line = "\"Alt+X\" 123 1 do something now";
        let (keystroke, is_global, action, _) = parse_line(line).expect("parse failed");
        assert_eq!(keystroke, "Alt+X");
        assert_eq!(is_global, true);
        assert_eq!(action, "do something now");
    }

    #[test]
    fn errors_when_missing_quote() {
        let line = "Ctrl k\" 0 0 action";
        assert!(parse_line(line).is_err());
    }
}

fn last_segment_after_unescaped_slash(s: &str) -> &str {
    let ci: Vec<(usize, char)> = s.char_indices().collect();
    // walk backward over the char-index pairs
    for i in (0..ci.len()).rev() {
        let (idx, ch) = ci[i];
        if ch != '/' {
            continue;
        }
        // if there's a char before this slash, check if it is a backslash
        if i == 0 {
            // slash at start -> nothing before it, so take everything after
            return &s[idx + ch.len_utf8()..];
        }
        let (_prev_idx, prev_ch) = ci[i - 1];
        if prev_ch == '\\' {
            // escaped slash -> skip it
            continue;
        }
        // found an unescaped '/'
        return &s[idx + ch.len_utf8()..];
    }
    // no unescaped slash found -> return full string
    s
}
