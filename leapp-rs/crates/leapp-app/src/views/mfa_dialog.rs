//! The question "MFA code for <session>?" that core asks before it gets a token for a session
//! with an MFA device, at a start or at a refresh.

use rune::{El, col, row, text};
use rune_kit::{Button, ButtonVariant, Dialog, Kit, TextInput};

use crate::Ui;

const DIALOG_WIDTH: f32 = 400.0;

/// Adds the dialog to `layer` when the question in front is an MFA question.
pub(crate) fn build(s: &Ui, k: &Kit, layer: &mut Vec<El>) {
    let Some(session) = s.pending_mfa() else {
        return;
    };
    let armed = s.question_armed();
    let body = col()
        .key("dlg-mfa")
        .w_full()
        .px(16.0)
        .pb(16.0)
        .gap(12.0)
        .child(
            text("Type the 6-digit code of your MFA device.".to_string())
                .font(k.ty.ui)
                .color(k.c.text_muted),
        )
        .child(
            // Secure: the element tree has no text for it, and `TextEdit` wipes its buffers.
            TextInput::new(s.mfa_code())
                .secure()
                .placeholder("MFA code")
                .label("MFA code")
                .width(None)
                .on_edit(|s: &mut Ui| s.mfa_code_mut())
                .on_submit(|s: &mut Ui, _tx: &mut rune::Tx| s.submit_mfa())
                .build(k)
                .key("fld-mfa-code"),
        )
        .child(
            row()
                .w_full()
                .pt(8.0)
                .gap(8.0)
                .justify_end()
                .child(
                    Button::new("Cancel")
                        .variant(ButtonVariant::Ghost)
                        .disabled(!armed)
                        .on_press(|s: &mut Ui| s.cancel_mfa())
                        .build(k)
                        .key("btn-mfa-cancel"),
                )
                .child(
                    Button::new("Submit")
                        .disabled(!s.can_submit_mfa())
                        .on_press(|s: &mut Ui| s.submit_mfa())
                        .build(k)
                        .key("btn-mfa-submit"),
                ),
        );

    Dialog::new("mfa", format!("MFA code for {session}"))
        .width(DIALOG_WIDTH)
        .fit()
        .open(true)
        .on_close(|s: &mut Ui| s.cancel_mfa())
        .child(body)
        .build(k, layer);
}
