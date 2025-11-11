use std::ffi::{CStr, CString};

use crate::DB_conf_item_s;

pub struct DBConfigurationItem {
    ptr: *mut DB_conf_item_s,
}

pub struct DBConfigurationItemIter {
    pub(crate) current: *mut DB_conf_item_s,
    pub(crate) key: CString,
    pub(crate) conf_find:
        unsafe extern "C" fn(*const i8, *mut DB_conf_item_s) -> *mut DB_conf_item_s,
}

impl Iterator for DBConfigurationItemIter {
    type Item = DBConfigurationItem;

    fn next(&mut self) -> Option<Self::Item> {
        if self.current.is_null() {
            return None;
        }

        let item = self.current;
        unsafe {
            self.current = (self.conf_find)(self.key.as_ptr(), self.current);
        }

        Some(DBConfigurationItem { ptr: item })
    }
}

impl DBConfigurationItem {
    pub fn key(&self) -> Option<&str> {
        unsafe {
            if (*self.ptr).key.is_null() {
                None
            } else {
                CStr::from_ptr((*self.ptr).key).to_str().ok()
            }
        }
    }

    pub fn value(&self) -> Option<&str> {
        unsafe {
            if (*self.ptr).value.is_null() {
                None
            } else {
                CStr::from_ptr((*self.ptr).value).to_str().ok()
            }
        }
    }
}
