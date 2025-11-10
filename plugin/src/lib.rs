use deadbeef_sys::*;
use once_cell::sync::Lazy;
use std::{ffi::c_int, ptr::null_mut, sync::Mutex};

#[macro_use]
mod utils;

mod plugin;
use plugin::*;

static PLUGIN: Lazy<Mutex<MiscPlugin>> = Lazy::new(|| {
    let x = DB_hotkeys_plugin_t {
        get_action_for_keycombo: Some(get_action_for_keycombo),
        get_name_for_keycode: None,
        reset: None,
        misc: DB_misc_t {
            plugin: DB_plugin_t {
                api_vmajor: 1,
                api_vminor: 0,
                version_major: 0,
                version_minor: 1,
                flags: DDB_PLUGIN_FLAG_LOGGING,
                type_: DB_PLUGIN_MISC as i32,
                id: c"hotkeys_rs".as_ptr(),
                name: c"Hotkeys plugin using portal (rust)".as_ptr(),
                descr: c"This is a new hotkeys plugin that uses XDG Portal for Global shortcut to support wayland".as_ptr(),
                copyright: concat!(include_str!("../../LICENSE"), "\0").as_ptr() as *const i8,
                website: c"https://saivert.com".as_ptr(),
                start: Some(plugin_start),
                stop: Some(plugin_stop),
                message: None,
                connect: None,
                get_actions: None,
                exec_cmdline: None,
                disconnect: None,
                command: None,
                configdialog: std::ptr::null(),
                reserved1: 0,
                reserved2: 0,
                reserved3: 0,
            },
        }

    };
    Mutex::new(MiscPlugin::new(x))
});

// DB_plugin_action_t *(*get_action_for_keycombo) (int key, int mods, int isglobal, ddb_action_context_t *ctx);

#[allow(unused)]
extern "C" fn get_action_for_keycombo(
    key: i32,
    mods: i32,
    isglobal: i32,
    ctx: *mut u32,
) -> *mut DB_plugin_action_t {
    debug!("ddb_hotkeys_rs::get_action_for_keycombo");
    if let Ok(p) = &mut PLUGIN.lock() {}
    null_mut()
}

extern "C" fn plugin_start() -> c_int {
    if let Ok(p) = &mut PLUGIN.lock() {
        p.plugin_start();
    }
    0
}

extern "C" fn plugin_stop() -> c_int {
    if let Ok(p) = &mut PLUGIN.lock() {
        p.plugin_stop();
    }
    0
}

// extern "C" fn message(msgid: u32, ctx: usize, p1: u32, p2: u32) -> c_int {
//     if let Ok(p) = PLUGIN.lock() {
//         p.message(msgid, ctx, p1, p2);
//     }
//     0
// }

#[no_mangle]
///
/// # Safety
/// This is requires since this is a plugin export function
pub unsafe extern "C" fn deadbeef_hotkeys_rust_load(
    api: *const DB_functions_t,
) -> *mut DB_plugin_t {
    tracing_subscriber::fmt::init();
    DeadBeef::init_from_ptr(api, &*PLUGIN.lock().unwrap())
}
