use crate::{utils::last_segment_after_unescaped_slash, utils::parse_line};
use ashpd::desktop::{
    global_shortcuts::{
        Activated, Deactivated, GlobalShortcuts, NewShortcut, Shortcut, ShortcutsChanged,
    },
    ResponseError, Session,
};
use async_lock::Mutex;
use deadbeef_sys::DeadBeef;
use futures_util::{
    future::Abortable,
    stream::{select_all, AbortRegistration, Stream, StreamExt},
};
use std::{collections::HashSet, str::FromStr, sync::Arc};

#[derive(Debug, Clone)]
struct RegisteredShortcut {
    id: String,
    activation: String,
}

#[derive(Debug)]
enum Event {
    Activated(Activated),
    Deactivated(Deactivated),
    ShortcutsChanged(ShortcutsChanged),
}

pub(crate) struct ShortcutHandler {
    session: Arc<Mutex<Option<Session<'static, GlobalShortcuts<'static>>>>>,
    abort_registration: std::cell::Cell<Option<AbortRegistration>>,
    triggers: Arc<Mutex<Vec<RegisteredShortcut>>>,
    activations: Arc<Mutex<HashSet<String>>>,
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

    pub async fn start_session(&self) -> ashpd::Result<()> {
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

    pub async fn stop(&self) {
        tracing::debug!("Aborting");

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
