//! The menu of a session row: its `⋯` button, or a right-click on the row, opens it.

use leapp_core::{Session, SessionKind, SessionStatus};
use rune::{El, Icon, Tx};
use rune_kit::{IconButton, IconButtonVariant, Kit, Menu, MenuItem, Placement};

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

/// The items, in this order: Start or Stop, Edit, Copy account number, Delete.
fn items(session: &Session) -> [MenuItem; 4] {
    let id = session.id;
    let toggle = match session.status {
        SessionStatus::Active => MenuItem::new("Stop session"),
        // A pending session can not start or stop now.
        SessionStatus::Pending => MenuItem::new("Start session").disabled(true),
        SessionStatus::Inactive => MenuItem::new("Start session"),
    }
    .on_select(move |s: &mut Ui| {
        s.close_menu();
        s.toggle(id);
    });
    let edit = MenuItem::new("Edit session").on_select(move |s: &mut Ui| {
        s.close_menu();
        s.open_edit(id);
    });
    let account = match &session.kind {
        SessionKind::IamUser { account, .. } => account.clone(),
        // Task 10 adds the account and the role ARN of a chained session.
        SessionKind::IamRoleChained { .. } => None,
    };
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
    [toggle, edit, copy, delete]
}
