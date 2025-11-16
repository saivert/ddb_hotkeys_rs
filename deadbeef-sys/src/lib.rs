#![allow(non_upper_case_globals)]
#![allow(non_camel_case_types)]
#![allow(non_snake_case)]
#![allow(dead_code)]
#![allow(clippy::all)]
#![allow(unnecessary_transmutes)]

use lossycstring::LossyCString;

use std::ptr::{self};
use thiserror::Error;

static mut DEADBEEF: Option<DeadBeef> = None;

#[allow(deref_nullptr)]
mod api {
    include!(concat!(env!("OUT_DIR"), "/bindings.rs"));
}
pub use api::*;

pub mod conf_item;
pub mod plugin;

use crate::{conf_item::DBConfigurationItemIter, plugin::PluginIter};

/// Main DeadBeef struct that encapsulates common DeadBeef API functions.
pub struct DeadBeef {
    pub(crate) ptr: *const DB_functions_t,
    pub(crate) plugin_ptr: *mut DB_plugin_t,
}

pub trait DBPlugin {
    fn get_plugin_ptr(&self) -> *const DB_plugin_t;
}

#[derive(Error, Debug)]
pub enum DB_TF_Error {
    #[error("Compile error")]
    CompileError,
    #[error("Evaluation error")]
    EvalError,
    #[error(transparent)]
    DBError(#[from] DB_Error),
}

#[derive(Error, Debug)]
pub enum DB_Error {
    #[error("Creation failed")]
    CreationFailed,
    #[error("No memory")]
    NoMemory,
}

impl DeadBeef {
    pub unsafe fn init_from_ptr(
        api: *const DB_functions_t,
        plugin: &impl DBPlugin,
    ) -> *mut DB_plugin_t {
        assert!(!api.is_null());
        let ptr = plugin.get_plugin_ptr() as *mut DB_plugin_t;
        DEADBEEF = Some(DeadBeef {
            ptr: api,
            plugin_ptr: ptr as *mut DB_plugin_t,
        });

        ptr
    }

    pub fn set_plugin_ptr(ptr: *mut DB_plugin_t) {
        let deadbeef = unsafe { DeadBeef::deadbeef() };
        deadbeef.plugin_ptr = ptr;
    }

    pub unsafe fn deadbeef() -> &'static mut DeadBeef {
        match DEADBEEF {
            Some(ref mut w) => w,
            None => panic!("Plugin wasn't initialized correctly"),
        }
    }

    #[inline]
    pub(crate) fn get(&self) -> &DB_functions_t {
        unsafe { &*self.ptr }
    }

    pub fn sendmessage(msg: u32, ctx: usize, p1: u32, p2: u32) -> i32 {
        let deadbeef = unsafe { DeadBeef::deadbeef() };

        let sendmessage = deadbeef.get().sendmessage.unwrap();

        unsafe { sendmessage(msg, ctx, p1, p2) }
    }

    pub fn log_detailed(layers: u32, msg: &str) {
        let deadbeef = unsafe { DeadBeef::deadbeef() };
        let log_detailed = deadbeef.get().log_detailed.unwrap();
        let msg = LossyCString::new(msg);
        unsafe {
            // Using transmute here to squash clippy warning about variadic function.
            let log_detailed_fn: extern "C" fn(*mut DB_plugin_t, u32, *const i8) =
                std::mem::transmute(log_detailed);
            log_detailed_fn(
                deadbeef.plugin_ptr as *mut DB_plugin_t,
                layers,
                msg.as_ptr(),
            );
        }
    }

    pub fn conf_get_str(item: impl AsRef<str>, default: impl AsRef<str>) -> String {
        let deadbeef = unsafe { DeadBeef::deadbeef() };

        let item = LossyCString::new(item);
        let default = LossyCString::new(default);
        let conf_get_str = deadbeef.get().conf_get_str.unwrap();
        let mut buf: Vec<u8> = vec![0; 4096];

        unsafe {
            conf_get_str(
                item.as_ptr(),
                default.as_ptr(),
                buf.as_mut_ptr() as *mut std::ffi::c_char,
                4096,
            );
        }

        let cstr = std::ffi::CStr::from_bytes_until_nul(&buf);
        return cstr
            .expect("null terminated string")
            .to_string_lossy()
            .into_owned();
    }

    pub fn plugins() -> PluginIter {
        let deadbeef = unsafe { DeadBeef::deadbeef() };
        let plug_get_list = deadbeef.get().plug_get_list.unwrap();
        let list = unsafe { plug_get_list() };
        if list.is_null() {
            return PluginIter {
                current: std::ptr::null_mut(),
            }; // Effectively returns an empty iterator.
        }
        PluginIter { current: list }
    }

    pub fn find_action_by_name(name: &str) -> Option<plugin::Action> {
        for plugin in Self::plugins() {
            for action in plugin.actions() {
                if let Some(act_name) = action.name() {
                    if act_name.eq_ignore_ascii_case(name) {
                        return Some(action);
                    }
                }
            }
        }
        None
    }

    pub fn call_action_by_name(name: &str) {
        if let Some(action) = Self::find_action_by_name(name) {
            action.call(DDB_ACTION_CTX_MAIN);
        }
    }

    pub fn conf_find_str(key: impl AsRef<str>) -> Option<DBConfigurationItemIter> {
        let deadbeef = unsafe { DeadBeef::deadbeef() };
        let conf_find = deadbeef.get().conf_find.unwrap();
        let key = LossyCString::new(key);
        let list = unsafe { conf_find(key.as_ptr(), std::ptr::null_mut()) };
        if list.is_null() {
            return None;
        }
        Some(DBConfigurationItemIter {
            current: list,
            key,
            conf_find,
        })
    }
}

pub struct PlItem {
    ptr: ptr::NonNull<DB_playItem_s>,
}

impl PlItem {
    pub fn from_raw(fromptr: *mut DB_playItem_s) -> Result<Self, DB_Error> {
        let ptr: ptr::NonNull<DB_playItem_s> =
            ptr::NonNull::new(fromptr).ok_or(DB_Error::CreationFailed)?;
        Ok(Self { ptr })
    }

    pub fn pl_item_unref(item: *mut DB_playItem_s) {
        let deadbeef = unsafe { DeadBeef::deadbeef() };
        let pl_item_unref = deadbeef.get().pl_item_unref.unwrap();

        unsafe {
            pl_item_unref(item);
        }
    }

    fn as_ptr(&self) -> *mut DB_playItem_s {
        self.ptr.as_ptr()
    }
}

impl std::ops::Drop for PlItem {
    fn drop(&mut self) {
        PlItem::pl_item_unref(self.ptr.as_ptr());
    }
}

#[derive(Copy, Clone, PartialEq, Eq)]
pub struct PlaybackState(ddb_playback_state_e);

impl PlaybackState {
    pub const Playing: Self = Self(DDB_PLAYBACK_STATE_PLAYING);
    pub const Stopped: Self = Self(DDB_PLAYBACK_STATE_STOPPED);
    pub const Paused: Self = Self(DDB_PLAYBACK_STATE_PAUSED);

    pub fn from_raw(raw: ddb_playback_state_e) -> Self {
        Self(raw)
    }

    pub fn as_raw(&self) -> ddb_playback_state_e {
        self.0
    }
}

impl std::fmt::Debug for PlaybackState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let name = format!(
            "PlaybackState::{}",
            match *self {
                Self::Playing => "Playing",
                Self::Paused => "Paused",
                Self::Stopped => "Stopped",
                _ => "Unknown",
            }
        );
        f.write_str(&name)
    }
}
