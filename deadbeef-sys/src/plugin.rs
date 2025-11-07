use std::ffi::CStr;

use crate::{DB_plugin_action_t, DB_plugin_t, ddb_action_context_e};

pub struct Plugin {
    ptr: *mut DB_plugin_t,
}

pub struct Action {
    ptr: *mut DB_plugin_action_t,
}
pub struct ActionIter {
    current: *mut DB_plugin_action_t,
}

impl Iterator for ActionIter {
    type Item = Action;

    fn next(&mut self) -> Option<Self::Item> {
        if self.current.is_null() {
            return None;
        }

        let action = self.current;
        unsafe {
            self.current = (*self.current).next;
        }

        Some(Action { ptr: action })
    }
}

pub struct PluginIter {
    pub(crate) current: *mut *mut DB_plugin_t,
}

impl Iterator for PluginIter {
    type Item = Plugin;

    fn next(&mut self) -> Option<Self::Item> {
        unsafe {
            let p = *self.current;
            if p.is_null() {
                None
            } else {
                self.current = self.current.add(1);
                Some(Plugin { ptr: p })
            }
        }
    }
}

impl Plugin {
    pub fn as_ptr(&self) -> *mut DB_plugin_t {
        self.ptr
    }

    pub fn name(&self) -> Option<&str> {
        unsafe {
            if (*self.ptr).name.is_null() {
                None
            } else {
                CStr::from_ptr((*self.ptr).name).to_str().ok()
            }
        }
    }

    pub fn actions(&self) -> ActionIter {
        unsafe {
            // let get_actions = (*self.ptr).get_actions;
            if let Some(get_actions) = (*self.ptr).get_actions {
                let first = get_actions(std::ptr::null_mut());
                ActionIter { current: first }
            } else {
                ActionIter { current: std::ptr::null_mut() }
            }
        }
    }
}

impl Action {
    pub fn as_ptr(&self) -> *mut DB_plugin_action_t {
        self.ptr
    }

    pub fn name(&self) -> Option<&str> {
        unsafe {
            if (*self.ptr).name.is_null() {
                None
            } else {
                CStr::from_ptr((*self.ptr).name).to_str().ok()
            }
        }
    }

    pub fn title(&self) -> Option<&str> {
        unsafe {
            if (*self.ptr).title.is_null() {
                None
            } else {
                CStr::from_ptr((*self.ptr).title).to_str().ok()
            }
        }
    }

    pub fn call(&self, context: ddb_action_context_e) {
        if let Some(callback2) = unsafe { (*self.ptr).callback2 } {
            unsafe {
                callback2(self.ptr, context);
            }
        }
    }
}
