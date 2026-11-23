//! The main window: a sidebar on the left; a toolbar and the session table on the right
//! (spec 14.1).

use leapp_core::{Session, SessionKind, SessionStatus, Workspace};
use rune::{Color, El, Icon, ScrollSpec, col, el, hairline, oklch, row, text};
use rune_kit::{IconButton, IconButtonVariant, Kit, Spinner};

use crate::Ui;

const SIDEBAR_WIDTH: f32 = 220.0;
const TOOLBAR_HEIGHT: f32 = 52.0;
const ROW_HEIGHT: f32 = 56.0;
const ROLE_WIDTH: f32 = 150.0;
const PROVIDER_WIDTH: f32 = 150.0;
const PROFILE_WIDTH: f32 = 160.0;
const REGION_WIDTH: f32 = 120.0;
const STATUS_WIDTH: f32 = 40.0;

/// The (+) of the toolbar: a circle with a plus in it.
static PLUS_CIRCLE: Icon = Icon {
    name: "leapp-plus-circle",
    view: 16.0,
    body: r#"<circle cx="8" cy="8" r="6.5" stroke="currentColor"/><path d="M8 5V11M5 8H11" stroke="currentColor" stroke-linecap="round"/>"#,
};

/// Leapp's IAM User chip colour.
fn orange() -> Color {
    oklch(0.75, 0.17, 60.0)
}

/// Leapp's region chip colour.
fn grey() -> Color {
    oklch(0.55, 0.0, 0.0)
}

pub(crate) fn view(s: &Ui, k: &Kit) -> El {
    let mut main = col()
        .grow(1.0)
        .h_full()
        .min_w(0.0)
        .fill(k.c.bg_base)
        .child(toolbar(k))
        .child(hairline(k.c.border_muted));
    main = if s.ws.sessions.is_empty() {
        main.child(empty_state(k))
    } else {
        main.child(table(&s.ws, k))
    };
    if let Some(message) = &s.error {
        main = main.child(error_toast(message, k));
    }
    row()
        .w_full()
        .h_full()
        .fill(k.c.page)
        .child(sidebar(k))
        .child(main)
}

fn sidebar(k: &Kit) -> El {
    let all_sessions = row()
        .key("nav-all-sessions")
        .role("button")
        .label("All Sessions")
        .h(32.0)
        .px(10.0)
        .center_y()
        .radius(k.radius.md)
        .fill(k.c.layer_02)
        .child(text("All Sessions").font(k.ty.ui).color(k.c.text_base));
    col()
        .key("sidebar")
        .w(SIDEBAR_WIDTH)
        .h_full()
        .pt(TOOLBAR_HEIGHT)
        .px(10.0)
        .gap(4.0)
        .fill(k.c.layer_01)
        .border_sides([0.0, 1.0, 0.0, 0.0], k.c.border_muted)
        .child(all_sessions)
}

fn toolbar(k: &Kit) -> El {
    let create = IconButton::new(&PLUS_CIRCLE)
        .variant(IconButtonVariant::Ghost)
        .label("Add session")
        .on_press(super::create_dialog::open)
        .build(k)
        .key("btn-create");
    row()
        .w_full()
        .h(TOOLBAR_HEIGHT)
        .px(16.0)
        .center_y()
        .child(create)
}

fn empty_state(k: &Kit) -> El {
    col().key("empty-state").grow(1.0).w_full().center().child(
        text("No sessions yet")
            .font(k.ty.ui_medium)
            .color(k.c.text_muted),
    )
}

/// Active sessions first, then the others. Each group in name order.
fn sorted(ws: &Workspace) -> Vec<&Session> {
    let mut sessions: Vec<&Session> = ws.sessions.iter().collect();
    sessions.sort_by(|a, b| {
        let not_active = |s: &Session| s.status != SessionStatus::Active;
        (not_active(a), &a.name).cmp(&(not_active(b), &b.name))
    });
    sessions
}

fn table(ws: &Workspace, k: &Kit) -> El {
    let header = row()
        .w_full()
        .h(44.0)
        .px(16.0)
        .center_y()
        .child(header_cell("Session", k).grow(1.0).min_w(0.0))
        .child(header_cell("Role", k).w(ROLE_WIDTH))
        .child(header_cell("Provider", k).w(PROVIDER_WIDTH))
        .child(header_cell("Named Profile", k).w(PROFILE_WIDTH))
        .child(header_cell("Region", k).w(REGION_WIDTH))
        .child(el().w(STATUS_WIDTH));
    let mut rows = col().w_full();
    for session in sorted(ws) {
        rows = rows.child(session_row(ws, session, k));
    }
    col()
        .key("table")
        .role("grid")
        .w_full()
        .grow(1.0)
        .min_h(0.0)
        .child(header)
        .child(hairline(k.c.border_muted))
        .child(
            rows.scroll(ScrollSpec {
                stick_bottom: false,
            })
            .grow(1.0)
            .min_h(0.0),
        )
}

fn header_cell(title: &str, k: &Kit) -> El {
    text(title.to_string()).font(k.ty.ui).color(k.c.text_muted)
}

fn session_row(ws: &Workspace, s: &Session, k: &Kit) -> El {
    let id = s.id;
    let profile = ws.profile_name(s.profile_id).unwrap_or_default();
    let name = text(s.name.clone())
        .key(format!("cell-name-{id}"))
        .font(k.ty.ui)
        .color(k.c.text_base)
        .truncate();
    let provider = chip(
        format!("cell-provider-{id}"),
        provider_label(&s.kind),
        orange(),
        10.0,
        k,
    );
    let region = chip(
        format!("cell-region-{id}"),
        &s.region.to_uppercase(),
        grey(),
        k.radius.sm,
        k,
    );
    row()
        .key(format!("row-{id}"))
        .role("row")
        .w_full()
        .h(ROW_HEIGHT)
        .px(16.0)
        .center_y()
        .border_sides([0.0, 0.0, 1.0, 0.0], k.c.border_muted)
        .child(row().grow(1.0).min_w(0.0).child(name))
        .child(el().w(ROLE_WIDTH))
        .child(row().w(PROVIDER_WIDTH).child(provider))
        .child(
            row().w(PROFILE_WIDTH).child(
                text(profile.to_string())
                    .key(format!("cell-profile-{id}"))
                    .font(k.ty.ui)
                    .color(k.c.text_muted)
                    .truncate(),
            ),
        )
        .child(row().w(REGION_WIDTH).child(region))
        .child(
            row()
                .w(STATUS_WIDTH)
                .justify_end()
                .child(status(id, s.status, k)),
        )
}

fn provider_label(kind: &SessionKind) -> &'static str {
    match kind {
        SessionKind::IamUser { .. } => "IAM User",
    }
}

/// A small coloured label. The key is on the text, so tests read the text by the key.
fn chip(key: String, label: &str, fill: Color, radius: f32, k: &Kit) -> El {
    row()
        .h(22.0)
        .px(8.0)
        .center_y()
        .radius(radius)
        .fill(fill)
        .child(
            text(label.to_string())
                .key(key)
                .font(k.ty.small_medium)
                .color(k.c.white),
        )
}

/// Grey dot (inactive), spinner (pending) or orange dot (active). The label says the status.
fn status(id: leapp_core::SessionId, status: SessionStatus, k: &Kit) -> El {
    let (label, inner) = match status {
        SessionStatus::Inactive => ("inactive", dot(k.c.layer_03)),
        SessionStatus::Pending => ("pending", Spinner::new().size(12.0).build(k)),
        SessionStatus::Active => ("active", dot(orange())),
    };
    row()
        .key(format!("status-{id}"))
        .role("status")
        .label(label)
        .size(12.0, 12.0)
        .center()
        .child(inner)
}

fn dot(fill: Color) -> El {
    el().size(12.0, 12.0).radius(6.0).fill(fill)
}

fn error_toast(message: &str, k: &Kit) -> El {
    row()
        .key("toast-error")
        .role("alert")
        .w_full()
        .px(16.0)
        .py(10.0)
        .center_y()
        .fill(k.c.danger.bg)
        .border_sides([1.0, 0.0, 0.0, 0.0], k.c.danger.border)
        .child(
            text(message.to_string())
                .font(k.ty.ui)
                .color(k.c.danger.fg)
                .grow(1.0)
                .min_w(0.0),
        )
        .child(
            text("Close")
                .key("toast-error-close")
                .role("button")
                .font(k.ty.ui_medium)
                .color(k.c.danger.fg)
                .on_press(|s: &mut Ui| s.error = None),
        )
}
