//! Generic guard against Bevy's classic change-detection footgun: taking
//! `&mut T` out of a `ResMut<T>` -- even via an ordinary deref coercion at a
//! call site that only reads -- marks the resource changed regardless of
//! whether anything was actually written. For resources gated on
//! `resource_changed` by expensive systems, an incidental `&mut` handed to
//! read-mostly code (typically egui UI) makes those systems run every frame
//! instead of only on real edits.
//!
//! [`Guarded<T>`] closes this off structurally: it only exposes read access
//! by default (`Deref`, no `DerefMut`), and requires mutation to go through
//! an explicit [`mutate`](Guarded::mutate)/[`mutate_if`](Guarded::mutate_if)/
//! [`bypass`](Guarded::bypass) call, so "I only needed a `&mut T` to satisfy
//! a signature" can no longer happen by accident.
//!
//! For widgets that bind `&mut` to a field every frame they're rendered
//! (checkboxes, dropdowns) regardless of user interaction, even an explicit
//! `.mutate()` call at the binding site would mark the resource changed on
//! every idle frame. Use [`edit_if_changed`] there instead: it renders
//! against a scratch copy and only reports back a value to commit if the
//! widget actually produced a different one.

use bevy::ecs::system::SystemParam;
use bevy::prelude::*;

#[derive(SystemParam)]
pub struct Guarded<'w, T: Resource> {
    inner: ResMut<'w, T>,
}

impl<'w, T: Resource> std::ops::Deref for Guarded<'w, T> {
    type Target = T;
    fn deref(&self) -> &T {
        &self.inner
    }
}

impl<'w, T: Resource> Guarded<'w, T> {
    /// The caller is vouching this is a genuine edit (e.g. behind a button
    /// click or other real user action) -- unconditionally marks the
    /// resource changed, same as ordinary `ResMut` access, just requiring it
    /// to be asked for explicitly rather than triggered by an incidental
    /// `&mut`.
    pub fn mutate(&mut self) -> &mut T {
        self.inner.set_changed();
        self.inner.bypass_change_detection()
    }

    /// For call sites that only know after the fact whether a real change
    /// happened (e.g. an idempotent sync pass like `sync_places`).
    pub fn mutate_if(&mut self, f: impl FnOnce(&mut T) -> bool) {
        if f(self.inner.bypass_change_detection()) {
            self.inner.set_changed();
        }
    }

    /// Escape hatch for mutations that should never be visible to change
    /// detection at all (e.g. populating a lazy cache stored on the resource).
    pub fn bypass(&mut self) -> &mut T {
        self.inner.bypass_change_detection()
    }
}

/// Renders `render` against a scratch copy of `current` and returns the
/// scratch value only if `render` actually produced something different --
/// for widgets (egui checkboxes, dropdowns, ...) that need `&mut T` to bind
/// to every frame they're shown, regardless of whether the user interacts
/// that frame. Keeps the widget's normal `&mut field` ergonomics locally
/// while leaving it to the caller to decide whether the result is worth a
/// real, change-detection-visible write (typically via `Guarded::mutate`).
pub fn edit_if_changed<T: PartialEq + Clone>(
    current: &T,
    render: impl FnOnce(&mut T),
) -> Option<T> {
    let mut scratch = current.clone();
    render(&mut scratch);
    (scratch != *current).then_some(scratch)
}
