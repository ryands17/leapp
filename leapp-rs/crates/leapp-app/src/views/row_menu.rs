//! The menu of a session row: its `⋯` button, or a right-click on the row, opens it.

use leapp_core::{Session, SessionKind, SessionStatus};
use rune::{El, Icon, Tx};
use rune_kit::{IconButton, IconButtonVariant, Kit, Menu, MenuItem, Placement};

use super::change_dialogs;
use crate::Ui;

/// The width of the column that holds the `⋯` button.
pub(crate) const MENU_BUTTON_WIDTH: f32 = 32.0;

/// Three dots in a row.
static ELLIPSIS: Icon = Icon {
    name: "leapp-ellipsis",
    view: 16.0,
    body: r#"<circle cx="3.5" cy="8" r="1.25" fill="currentColor"/><circle cx="8" cy="8" r="1.25" fill="currentColor"/><circle cx="12.5" cy="8" r="1.25" fill="currentColor"/>"#,
};

/// The `⋯` button of the row of `session`, keyed `row-menu-<id>`. If the menu of this session is
/// open, also adds the menu (keyed `menu-<id>`) to `layer`.
pub(crate) fn build(s: &Ui, session: &Session, k: &Kit, layer: &mut Vec<El>) -> El {
    let id = session.id;
    let button = IconButton::new(&ELLIPSIS)
        .variant(IconButtonVariant::Ghost)
        .label("Session actions")
        .on_press(move |s: &mut Ui| s.open_menu(id, None))
        .build(k)
        .key(format!("row-menu-{id}"));
    let open = s.row_menu().filter(|m| m.id == id);
    let mut menu = Menu::new(format!("menu-{id}"))
        .trigger(button)
        .open(open.is_some())
        .placement(Placement::BottomEnd)
        .on_close(|s: &mut Ui| s.close_menu());
    if let Some(p) = open.and_then(|m| m.at) {
        menu = menu.at(p.x, p.y);
    }
    if open.is_some() {
        for item in items(session) {
            menu = menu.item(item);
        }
    }
    menu.build(k, layer)
}

/// The items, in this order (spec 14.3 without the F6 and F7 items): Start or Stop, Change
/// region, Change named profile, Copy account number, Copy role ARN (chained sessions only),
/// Edit, Delete.
fn items(session: &Session) -> Vec<MenuItem> {
    let id = session.id;
    // A pending session can not start, stop or change now.
    let pending = session.status == SessionStatus::Pending;
    let toggle = match session.status {
        SessionStatus::Active => MenuItem::new("Stop session"),
        SessionStatus::Pending | SessionStatus::Inactive => MenuItem::new("Start session"),
    }
    .disabled(pending)
    .on_select(move |s: &mut Ui| {
        s.close_menu();
        s.toggle(id);
    });
    let region = MenuItem::new("Change region\u{2026}")
        .disabled(pending)
        .on_select(move |s: &mut Ui| {
            s.close_menu();
            change_dialogs::open_region(s, id);
        });
    let profile = MenuItem::new("Change named profile\u{2026}")
        .disabled(pending)
        .on_select(move |s: &mut Ui| {
            s.close_menu();
            change_dialogs::open_profile(s, id);
        });
    let edit = MenuItem::new("Edit session").on_select(move |s: &mut Ui| {
        s.close_menu();
        s.open_edit(id);
    });
    let account = session.account();
    let copy = MenuItem::new("Copy account number")
        .disabled(account.is_none())
        .on_select_tx(move |s: &mut Ui, tx: &mut Tx| {
            s.close_menu();
            if let Some(account) = &account {
                tx.copy(account.clone());
            }
        });
    let delete = MenuItem::new("Delete session")
        .danger(true)
        .on_select(move |s: &mut Ui| {
            s.close_menu();
            s.ask_delete(id);
        });
    let role_arn = match &session.kind {
        SessionKind::IamRoleChained { role_arn, .. } => Some(role_arn.clone()),
        SessionKind::IamUser { .. } => None,
    };
    let copy_role_arn = role_arn.map(|arn| {
        MenuItem::new("Copy role ARN").on_select_tx(move |s: &mut Ui, tx: &mut Tx| {
            s.close_menu();
            tx.copy(arn.clone());
        })
    });
    let mut items = vec![toggle, region, profile, copy];
    items.extend(copy_role_arn);
    items.extend([edit, delete]);
    items
}
