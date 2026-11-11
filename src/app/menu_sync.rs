//! What the macOS menu bar (`macos_menu.rs`) must change to show a new menu list, and the text
//! it gives muda. Kept apart from AppKit so it is tested on every platform.

use super::menu_model::{MenuBar, MenuEntry, MenuItem};

/// What the menu bar must do.
#[derive(Debug, PartialEq, Eq)]
pub(super) enum Sync {
    /// The top-level menus are different: build the whole menu again.
    All,
    /// One change for each top-level menu, in order.
    Menus(Vec<MenuChange>),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum MenuChange {
    Same,
    /// Only labels, on and off, ticks or shortcuts differ: change the items in place.
    State,
    /// Entries were added, removed or changed kind: build this menu again.
    Rebuild,
}

pub(super) fn diff(old: &MenuBar, new: &MenuBar) -> Sync {
    if old.menus.len() != new.menus.len()
        || old
            .menus
            .iter()
            .zip(&new.menus)
            .any(|(old, new)| old.role != new.role)
    {
        return Sync::All;
    }
    Sync::Menus(
        old.menus
            .iter()
            .zip(&new.menus)
            .map(|(old, new)| {
                if old == new {
                    MenuChange::Same
                } else if same_shape(&old.entries, &new.entries) {
                    MenuChange::State
                } else {
                    MenuChange::Rebuild
                }
            })
            .collect(),
    )
}

/// Whether two lists differ only in what the menu bar can change in place. Items must do the
/// same thing, as the menu bar maps each item to its action when it builds it.
fn same_shape(old: &[MenuEntry], new: &[MenuEntry]) -> bool {
    old.len() == new.len()
        && old.iter().zip(new).all(|pair| match pair {
            (MenuEntry::Item(old), MenuEntry::Item(new)) => {
                old.action == new.action
                    && old.checked.is_some() == new.checked.is_some()
                    // muda cannot clear a key equivalent in place, only set one.
                    && !(old.chord.is_some() && new.chord.is_none())
            }
            (MenuEntry::Submenu { entries: old, .. }, MenuEntry::Submenu { entries: new, .. }) => {
                same_shape(old, new)
            }
            (MenuEntry::Separator, MenuEntry::Separator)
            | (MenuEntry::Placeholder(_), MenuEntry::Placeholder(_)) => true,
            (MenuEntry::System { item: old, .. }, MenuEntry::System { item: new, .. }) => {
                old == new
            }
            _ => false,
        })
}

/// A label as muda wants it. muda reads `&` as a mnemonic marker and drops it, so a literal
/// `&` (Black & White, a plugin's label) is written `&&`.
pub(super) fn menu_text(label: &str) -> String {
    label.replace('&', "&&")
}

/// The item at `path`: a top-level menu's index, then an index into each submenu on the way.
/// The menu bar records each native item's path, so an event always finds the item as it is now.
pub(super) fn item_at<'a>(bar: &'a MenuBar, path: &[usize]) -> Option<&'a MenuItem> {
    let (&top, rest) = path.split_first()?;
    let (&last, submenus) = rest.split_last()?;
    let mut entries = bar.menus.get(top)?.entries.as_slice();
    for &index in submenus {
        let MenuEntry::Submenu {
            entries: children, ..
        } = entries.get(index)?
        else {
            return None;
        };
        entries = children;
    }
    match entries.get(last)? {
        MenuEntry::Item(item) => Some(item),
        _ => None,
    }
}
