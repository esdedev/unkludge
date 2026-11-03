//! What screen readers see. The window draws everything itself, so it has no controls for them to
//! find: this tells them about each item instead, through the older accessibility interface, which
//! UI Automation also reads. Children are the items of the last paint, numbered from 1, as this
//! interface wants.

use std::cell::RefCell;
use std::mem::ManuallyDrop;
use std::rc::Rc;

use windows::Win32::Foundation::{
    DISP_E_MEMBERNOTFOUND, E_INVALIDARG, E_NOTIMPL, HWND, LPARAM, POINT, RECT, S_FALSE, WPARAM,
};
use windows::Win32::Graphics::Gdi::{ClientToScreen, ScreenToClient};
use windows::Win32::System::Com::{
    DISPATCH_FLAGS, DISPPARAMS, EXCEPINFO, IDispatch, IDispatch_Impl, ITypeInfo,
};
use windows::Win32::System::Variant::{
    VARENUM, VARIANT, VARIANT_0, VARIANT_0_0, VARIANT_0_0_0, VT_EMPTY, VT_I4,
};
use windows::Win32::UI::Accessibility::{
    AccessibleObjectFromWindow, IAccessible, IAccessible_Impl, NotifyWinEvent,
    ROLE_SYSTEM_CHECKBUTTON, ROLE_SYSTEM_CLIENT, ROLE_SYSTEM_LINK, ROLE_SYSTEM_PUSHBUTTON,
    ROLE_SYSTEM_RADIOBUTTON, ROLE_SYSTEM_SLIDER, ROLE_SYSTEM_STATICTEXT,
};
use windows::Win32::UI::WindowsAndMessaging::{
    EVENT_OBJECT_NAMECHANGE, EVENT_OBJECT_REORDER, EVENT_OBJECT_STATECHANGE,
    EVENT_OBJECT_VALUECHANGE, GetClientRect, OBJID_CLIENT, OBJID_WINDOW, PostMessageW,
    STATE_SYSTEM_CHECKED, STATE_SYSTEM_FOCUSED,
};
use windows::core::{BSTR, Error, GUID, Interface, PCWSTR, Result, implement};

use super::window::WM_INVOKE_ITEM;
use super::{App, Item, Role, Shared};

/// The id that means the window's whole client area rather than one item.
const SELF: i32 = 0;
const NAV_NEXT: i32 = 5;
const NAV_PREVIOUS: i32 = 6;
const NAV_FIRST_CHILD: i32 = 7;
const NAV_LAST_CHILD: i32 = 8;
/// These two the bindings only have with another type, for another purpose.
const STATE_FOCUSABLE: u32 = 0x10_0000;
const STATE_UNAVAILABLE: u32 = 0x1;
const STATE_READ_ONLY: u32 = 0x40;

pub fn create(hwnd: HWND, shared: Rc<RefCell<Shared>>) -> IAccessible {
    Accessible { hwnd, shared }.into()
}

// Not agile: by default the macro marks an object as callable from any thread, and screen readers
// do call from theirs. This one shares `Rc<RefCell<Shared>>` with the window, so calls have to wait
// their turn on the window's thread, which is what COM does for an object that isn't agile. Without
// this, a call arriving during a paint panics with "already borrowed".
#[implement(IAccessible, Agile = false)]
struct Accessible {
    hwnd: HWND,
    shared: Rc<RefCell<Shared>>,
}

/// How an item differs from how it was in the last paint.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Change {
    Name,
    Value,
    State,
}

/// What's different from one paint to the next, by each item's place in the new list. An item that
/// has only just appeared has nothing to compare with: [`same_items`] says the list changed.
pub fn changes(before: &[Item], after: &[Item]) -> Vec<(usize, Change)> {
    let mut found = Vec::new();
    for (at, item) in after.iter().enumerate() {
        let Some(old) = before.iter().find(|old| old.hit == item.hit) else {
            continue;
        };
        if old.name != item.name {
            found.push((at, Change::Name));
        }
        if old.value != item.value {
            found.push((at, Change::Value));
        }
        if old.on != item.on || old.enabled != item.enabled {
            found.push((at, Change::State));
        }
    }
    found
}

/// Whether both lists have the same items in the same order.
pub fn same_items(before: &[Item], after: &[Item]) -> bool {
    before.len() == after.len()
        && before
            .iter()
            .zip(after)
            .all(|(old, new)| old.hit == new.hit)
}

impl App {
    /// Tells a screen reader that's listening what changed since the last paint.
    pub fn announce_changes(&mut self) {
        let shared = self.shared.borrow();
        let notify = |event: u32, child: i32| {
            // SAFETY: the handle is this window's own, and the rest are plain numbers.
            unsafe { NotifyWinEvent(event, self.hwnd, OBJID_CLIENT.0, child) };
        };
        if !same_items(&self.last_items, &shared.items) {
            notify(EVENT_OBJECT_REORDER, SELF);
        }
        for (at, change) in changes(&self.last_items, &shared.items) {
            let event = match change {
                Change::Name => EVENT_OBJECT_NAMECHANGE,
                Change::Value => EVENT_OBJECT_VALUECHANGE,
                Change::State => EVENT_OBJECT_STATECHANGE,
            };
            notify(event, at as i32 + 1);
        }
        let items = shared.items.clone();
        drop(shared);
        self.last_items = items;
    }
}

fn variant_i4(value: i32) -> VARIANT {
    typed_variant(VT_I4, VARIANT_0_0_0 { lVal: value })
}

fn variant_empty() -> VARIANT {
    typed_variant(VT_EMPTY, VARIANT_0_0_0 { lVal: 0 })
}

fn typed_variant(vt: VARENUM, value: VARIANT_0_0_0) -> VARIANT {
    VARIANT {
        Anonymous: VARIANT_0 {
            Anonymous: ManuallyDrop::new(VARIANT_0_0 {
                vt,
                wReserved1: 0,
                wReserved2: 0,
                wReserved3: 0,
                Anonymous: value,
            }),
        },
    }
}

/// Which child a call is about.
fn child_id(variant: &VARIANT) -> Result<i32> {
    // SAFETY: `vt` is valid whatever the union holds, and it says that `lVal` is.
    unsafe {
        let inner = &variant.Anonymous.Anonymous;
        if inner.vt == VT_I4 {
            Ok(inner.Anonymous.lVal)
        } else {
            Err(E_INVALIDARG.into())
        }
    }
}

/// Where a move leads, among children numbered from 1. Items have no children of their own, and the
/// window's siblings are the system's to say.
fn navigate(direction: i32, start: i32, count: i32) -> Option<i32> {
    let target = match (direction, start) {
        (NAV_FIRST_CHILD, SELF) => 1,
        (NAV_LAST_CHILD, SELF) => count,
        (NAV_NEXT, at) if at != SELF => at + 1,
        (NAV_PREVIOUS, at) if at != SELF => at - 1,
        _ => return None,
    };
    (1..=count).contains(&target).then_some(target)
}

fn role_of(role: Role) -> u32 {
    match role {
        Role::Button => ROLE_SYSTEM_PUSHBUTTON,
        Role::Toggle => ROLE_SYSTEM_CHECKBUTTON,
        Role::Choice => ROLE_SYSTEM_RADIOBUTTON,
        Role::Slider => ROLE_SYSTEM_SLIDER,
        Role::Link => ROLE_SYSTEM_LINK,
        Role::Text => ROLE_SYSTEM_STATICTEXT,
    }
}

impl Accessible {
    /// The item a call is about, or `None` for the window itself.
    fn with_item<T>(
        &self,
        variant: &VARIANT,
        on_self: impl FnOnce() -> Result<T>,
        on_item: impl FnOnce(&Item, &Shared) -> Result<T>,
    ) -> Result<T> {
        let id = child_id(variant)?;
        if id == SELF {
            return on_self();
        }
        let shared = self
            .shared
            .try_borrow()
            .map_err(|_| Error::from(E_NOTIMPL))?;
        let item = usize::try_from(id - 1)
            .ok()
            .and_then(|at| shared.items.get(at))
            .ok_or_else(|| Error::from(E_INVALIDARG))?;
        on_item(item, &shared)
    }
}

impl IAccessible_Impl for Accessible_Impl {
    fn accParent(&self) -> Result<IDispatch> {
        let mut parent = std::ptr::null_mut();
        // SAFETY: the out pointer is valid for the call, which hands over a reference on success.
        unsafe {
            AccessibleObjectFromWindow(
                self.hwnd,
                OBJID_WINDOW.0 as u32,
                &IDispatch::IID,
                &mut parent,
            )?;
            Ok(IDispatch::from_raw(parent))
        }
    }

    fn accChildCount(&self) -> Result<i32> {
        let shared = self
            .shared
            .try_borrow()
            .map_err(|_| Error::from(E_NOTIMPL))?;
        Ok(shared.items.len() as i32)
    }

    fn get_accChild(&self, _child: &VARIANT) -> Result<IDispatch> {
        // Items aren't objects of their own: the caller asks this one about them.
        Err(S_FALSE.into())
    }

    fn get_accName(&self, child: &VARIANT) -> Result<BSTR> {
        self.with_item(
            child,
            || Ok(BSTR::from("Unkludge")),
            |item, _| Ok(BSTR::from(item.name.as_str())),
        )
    }

    fn get_accValue(&self, child: &VARIANT) -> Result<BSTR> {
        self.with_item(
            child,
            || Err(DISP_E_MEMBERNOTFOUND.into()),
            |item, _| match &item.value {
                Some(value) => Ok(BSTR::from(value.as_str())),
                None => Err(DISP_E_MEMBERNOTFOUND.into()),
            },
        )
    }

    fn get_accDescription(&self, _child: &VARIANT) -> Result<BSTR> {
        Err(DISP_E_MEMBERNOTFOUND.into())
    }

    fn get_accRole(&self, child: &VARIANT) -> Result<VARIANT> {
        self.with_item(
            child,
            || Ok(variant_i4(ROLE_SYSTEM_CLIENT as i32)),
            |item, _| Ok(variant_i4(role_of(item.role) as i32)),
        )
    }

    fn get_accState(&self, child: &VARIANT) -> Result<VARIANT> {
        self.with_item(
            child,
            || Ok(variant_i4(0)),
            |item, shared| {
                let mut state = 0;
                if item.role == Role::Text {
                    state |= STATE_READ_ONLY;
                } else if item.enabled {
                    state |= STATE_FOCUSABLE;
                } else {
                    state |= STATE_UNAVAILABLE;
                }
                if item.on {
                    state |= STATE_SYSTEM_CHECKED;
                }
                if shared.focus == Some(item.hit) {
                    state |= STATE_SYSTEM_FOCUSED;
                }
                Ok(variant_i4(state as i32))
            },
        )
    }

    fn get_accHelp(&self, _child: &VARIANT) -> Result<BSTR> {
        Err(DISP_E_MEMBERNOTFOUND.into())
    }

    fn get_accHelpTopic(&self, _file: *mut BSTR, _child: &VARIANT) -> Result<i32> {
        Err(DISP_E_MEMBERNOTFOUND.into())
    }

    fn get_accKeyboardShortcut(&self, _child: &VARIANT) -> Result<BSTR> {
        Err(DISP_E_MEMBERNOTFOUND.into())
    }

    fn accFocus(&self) -> Result<VARIANT> {
        let shared = self
            .shared
            .try_borrow()
            .map_err(|_| Error::from(E_NOTIMPL))?;
        let at = shared
            .focus
            .and_then(|focus| shared.items.iter().position(|item| item.hit == focus));
        Ok(at.map_or_else(variant_empty, |at| variant_i4(at as i32 + 1)))
    }

    fn accSelection(&self) -> Result<VARIANT> {
        Ok(variant_empty())
    }

    fn get_accDefaultAction(&self, _child: &VARIANT) -> Result<BSTR> {
        Err(DISP_E_MEMBERNOTFOUND.into())
    }

    fn accSelect(&self, _flags: i32, _child: &VARIANT) -> Result<()> {
        Err(DISP_E_MEMBERNOTFOUND.into())
    }

    fn accLocation(
        &self,
        left: *mut i32,
        top: *mut i32,
        width: *mut i32,
        height: *mut i32,
        child: &VARIANT,
    ) -> Result<()> {
        let hwnd = self.hwnd;
        let (x, y, w, h) = self.with_item(
            child,
            || {
                let mut rect = RECT::default();
                // SAFETY: the rectangle is written by the call.
                unsafe { GetClientRect(hwnd, &mut rect)? };
                Ok((0.0, 0.0, rect.right as f32, rect.bottom as f32))
            },
            |item, shared| {
                let k = shared.scale;
                let r = item.rect;
                Ok((r.x0 * k, r.y0 * k, r.w() * k, r.h() * k))
            },
        )?;
        let mut origin = POINT {
            x: x as i32,
            y: y as i32,
        };
        // SAFETY: the point is read and written by the call. The caller's pointers are checked for
        // null, and are this interface's to write through otherwise.
        unsafe {
            let _ = ClientToScreen(hwnd, &mut origin);
            if left.is_null() || top.is_null() || width.is_null() || height.is_null() {
                return Err(E_INVALIDARG.into());
            }
            *left = origin.x;
            *top = origin.y;
            *width = w as i32;
            *height = h as i32;
        }
        Ok(())
    }

    fn accNavigate(&self, direction: i32, start: &VARIANT) -> Result<VARIANT> {
        let shared = self
            .shared
            .try_borrow()
            .map_err(|_| Error::from(E_NOTIMPL))?;
        let found = navigate(direction, child_id(start)?, shared.items.len() as i32);
        Ok(found.map_or_else(variant_empty, variant_i4))
    }

    fn accHitTest(&self, x: i32, y: i32) -> Result<VARIANT> {
        let mut point = POINT { x, y };
        // SAFETY: the point is read and written by the call.
        unsafe {
            let _ = ScreenToClient(self.hwnd, &mut point);
        }
        let shared = self
            .shared
            .try_borrow()
            .map_err(|_| Error::from(E_NOTIMPL))?;
        let k = shared.scale.max(0.01);
        let (x, y) = (point.x as f32 / k, point.y as f32 / k);
        let found = shared
            .items
            .iter()
            .rposition(|item| item.rect.contains(x, y));
        Ok(variant_i4(found.map_or(SELF, |at| at as i32 + 1)))
    }

    fn accDoDefaultAction(&self, child: &VARIANT) -> Result<()> {
        let id = child_id(child)?;
        let at = usize::try_from(id - 1).map_err(|_| Error::from(E_INVALIDARG))?;
        // Through the message queue, as the window may be in the middle of handling something.
        // SAFETY: posting takes plain numbers, and fails harmlessly if the window has gone.
        unsafe {
            PostMessageW(Some(self.hwnd), WM_INVOKE_ITEM, WPARAM(at), LPARAM(0))?;
        }
        Ok(())
    }

    fn put_accName(&self, _child: &VARIANT, _name: &BSTR) -> Result<()> {
        Err(DISP_E_MEMBERNOTFOUND.into())
    }

    fn put_accValue(&self, _child: &VARIANT, _value: &BSTR) -> Result<()> {
        Err(DISP_E_MEMBERNOTFOUND.into())
    }
}

impl IDispatch_Impl for Accessible_Impl {
    fn GetTypeInfoCount(&self) -> Result<u32> {
        Ok(0)
    }

    fn GetTypeInfo(&self, _index: u32, _lcid: u32) -> Result<ITypeInfo> {
        Err(E_NOTIMPL.into())
    }

    fn GetIDsOfNames(
        &self,
        _iid: *const GUID,
        _names: *const PCWSTR,
        _count: u32,
        _lcid: u32,
        _ids: *mut i32,
    ) -> Result<()> {
        Err(E_NOTIMPL.into())
    }

    fn Invoke(
        &self,
        _member: i32,
        _iid: *const GUID,
        _lcid: u32,
        _flags: DISPATCH_FLAGS,
        _params: *const DISPPARAMS,
        _result: *mut VARIANT,
        _exception: *mut EXCEPINFO,
        _arg_error: *mut u32,
    ) -> Result<()> {
        Err(E_NOTIMPL.into())
    }
}

#[cfg(test)]
mod tests {
    use super::super::Hit;
    use super::super::gfx::R;
    use super::*;
    use windows::Win32::System::Com::IAgileObject;

    /// The object shares an `Rc` with the window, so it must never be called from another thread.
    /// COM decides that by asking for this interface: having it means callers may skip the
    /// apartment and call straight in.
    #[test]
    fn calls_have_to_come_through_the_windows_thread() {
        let shared = Rc::new(RefCell::new(Shared::default()));
        let accessible = create(HWND::default(), shared);
        assert!(accessible.cast::<IAgileObject>().is_err());
    }

    fn item(hit: Hit, name: &str) -> Item {
        Item {
            rect: R::default(),
            hit,
            name: name.to_string(),
            value: None,
            role: Role::Button,
            on: false,
            enabled: true,
        }
    }

    #[test]
    fn nothing_different_means_nothing_to_tell() {
        let items = [item(Hit::Report, "a"), item(Hit::Upload, "b")];
        assert_eq!(changes(&items, &items), []);
        assert!(same_items(&items, &items));
    }

    #[test]
    fn a_new_name_a_value_and_a_state_are_each_told_by_the_items_new_place() {
        let before = [item(Hit::Report, "a"), item(Hit::Brightness, "b")];
        let mut name = before.clone();
        name[1].name = "c".to_string();
        assert_eq!(changes(&before, &name), [(1, Change::Name)]);
        let mut value = before.clone();
        value[0].value = Some("60 %".to_string());
        assert_eq!(changes(&before, &value), [(0, Change::Value)]);
        let mut on = before.clone();
        on[1].on = true;
        assert_eq!(changes(&before, &on), [(1, Change::State)]);
        let mut enabled = before.clone();
        enabled[0].enabled = false;
        assert_eq!(changes(&before, &enabled), [(0, Change::State)]);
    }

    #[test]
    fn an_item_is_followed_to_where_it_moved() {
        let before = [item(Hit::Report, "a")];
        let after = [item(Hit::Upload, "b"), item(Hit::Report, "c")];
        assert_eq!(changes(&before, &after), [(1, Change::Name)]);
        assert!(!same_items(&before, &after));
    }

    #[test]
    fn items_coming_and_going_change_the_list_and_nothing_else() {
        let before = [item(Hit::Report, "a")];
        assert_eq!(changes(&before, &[]), []);
        assert_eq!(changes(&[], &before), []);
        assert!(!same_items(&[], &before));
    }

    #[test]
    fn a_child_is_read_from_its_variant() {
        assert_eq!(child_id(&variant_i4(7)).unwrap(), 7);
        assert!(child_id(&variant_empty()).is_err());
    }

    #[test]
    fn moves_go_among_the_children() {
        assert_eq!(navigate(NAV_FIRST_CHILD, SELF, 3), Some(1));
        assert_eq!(navigate(NAV_LAST_CHILD, SELF, 3), Some(3));
        assert_eq!(navigate(NAV_NEXT, 1, 3), Some(2));
        assert_eq!(navigate(NAV_PREVIOUS, 2, 3), Some(1));
    }

    #[test]
    fn moves_stop_at_the_ends_and_at_nothing_to_go_to() {
        assert_eq!(navigate(NAV_NEXT, 3, 3), None);
        assert_eq!(navigate(NAV_PREVIOUS, 1, 3), None);
        assert_eq!(navigate(NAV_FIRST_CHILD, SELF, 0), None);
        assert_eq!(navigate(NAV_NEXT, SELF, 3), None);
        assert_eq!(navigate(NAV_FIRST_CHILD, 2, 3), None);
    }

    #[test]
    fn every_kind_of_item_has_a_role() {
        assert_eq!(role_of(Role::Button), ROLE_SYSTEM_PUSHBUTTON);
        assert_eq!(role_of(Role::Toggle), ROLE_SYSTEM_CHECKBUTTON);
        assert_eq!(role_of(Role::Choice), ROLE_SYSTEM_RADIOBUTTON);
        assert_eq!(role_of(Role::Slider), ROLE_SYSTEM_SLIDER);
        assert_eq!(role_of(Role::Link), ROLE_SYSTEM_LINK);
        assert_eq!(role_of(Role::Text), ROLE_SYSTEM_STATICTEXT);
    }
}
