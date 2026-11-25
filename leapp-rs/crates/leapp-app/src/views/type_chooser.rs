//! The first step of "Add session": the user chooses the session type (spec 14.5). F23-R12: only
//! IAM User and IAM Role Chained; Single Sign-On comes with F4.

use rune::{El, col};
use rune_kit::{Button, ButtonVariant, Dialog, Kit};

use super::{chained_dialog, create_dialog};
use crate::Ui;

const DIALOG_WIDTH: f32 = 360.0;

/// Opens the type chooser.
pub(crate) fn open(s: &mut Ui) {
    s.choosing_type = true;
}

fn close(s: &mut Ui) {
    s.choosing_type = false;
}

fn choose_iam_user(s: &mut Ui) {
    close(s);
    create_dialog::open(s);
}

fn choose_iam_role_chained(s: &mut Ui) {
    close(s);
    chained_dialog::open(s);
}

/// Adds the chooser to `layer` when it is open.
pub(crate) fn build(s: &Ui, k: &Kit, layer: &mut Vec<El>) {
    if !s.choosing_type {
        return;
    }
    let option = |label: &str, key: &str, on: fn(&mut Ui)| {
        Button::new(label)
            .variant(ButtonVariant::Outline)
            .on_press(on)
            .build(k)
            .key(key)
            .w_full()
    };
    let body = col()
        .key("dlg-choose-type")
        .w_full()
        .px(16.0)
        .pb(16.0)
        .gap(8.0)
        .child(option("AWS IAM User", "btn-type-iam-user", choose_iam_user))
        .child(option(
            "AWS IAM Role Chained",
            "btn-type-iam-role-chained",
            choose_iam_role_chained,
        ));
    // The rune-kit `Dialog` keys its panel `<key>-dialog`.
    Dialog::new("dlg-type", "Add session")
        .width(DIALOG_WIDTH)
        .fit()
        .open(true)
        .on_close(close)
        .child(body)
        .build(k, layer);
}
