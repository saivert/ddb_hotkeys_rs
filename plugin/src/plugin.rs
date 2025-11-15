use crate::{keysyms::parse_shortcut, shortcuthandler::ShortcutHandler, utils::parse_line, *};
use async_lock::Mutex;
use futures_util::future::AbortHandle;
use std::{sync::Arc, thread};

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
}

impl PluginThread {
    pub fn new(plugin: Arc<Mutex<ShortcutHandler>>) -> Self {
        Self {
            handle: thread::spawn(move || thread_main(plugin)),
        }
    }

    pub fn join(self) -> thread::Result<()> {
        self.handle.join()
    }
}

impl DBPlugin for MiscPlugin {
    fn get_plugin_ptr(&self) -> *const DB_plugin_t {
        &self.plugin as *const DB_hotkeys_plugin_t as *const DB_plugin_t
    }
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
    }

    pub fn plugin_stop(&mut self) {
        self.abort_handle
            .lock_blocking()
            .take()
            .expect("Abort handle")
            .abort();

        tracing::debug!("Waiting for shortcut handler to stop");
        smol::block_on(async {
            self.shortcut_handler.lock().await.stop().await;
            tracing::debug!("Stopped session.");
        });


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

    pub fn get_action_for_keycombo(
        &mut self,
        key: i32,
        mods: i32,
        isglobal: i32,
    ) -> Option<(ddb_action_context_t, *mut DB_plugin_action_t)> {
        let act = self
            .commands
            .iter()
            .find(|x| x.isglobal == isglobal && x.keycode == key && x.modifier == mods);

        act.map(|x| (x.ctx, x.action))
    }

    // #[allow(unused)]
    // pub fn message(&self, msgid: u32, ctx: usize, p1: u32, p2: u32) {
    //     match msgid {
    //         _ => {}
    //     }
    // }
}

fn thread_main(plugin: Arc<Mutex<ShortcutHandler>>) {
    smol::block_on(async {
        tracing::debug!("Plugin thread received Start message");
        if !plugin.lock().await.start_session().await.is_ok() {
            tracing::error!("Plugin session failed to start");
        }

        // while let Ok(msg) = receiver.recv().await {
        //     match msg {
        //         ThreadMessage::Terminate => {
        //             tracing::debug!("Plugin thread terminating...");
        //             plugin.lock().await.stop().await;
        //         }
        //         ThreadMessage::Start => {
        //             tracing::debug!("Plugin thread received Start message");
        //             if !plugin.lock().await.start_session().await.is_ok() {
        //                 tracing::debug!("Plugin session failed to start");
        //             }
        //         }
        //     }
        // }
    });
}
