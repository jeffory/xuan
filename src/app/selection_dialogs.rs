//! Select → Expand… and Contract…, after Compositor's selection amount prompt.
use std::sync::Arc;

use anyhow::Context as _;
use xuan::{i18n::tr, selection_ops};

use super::{Dialog, EditorApp, widgets};

/// Which amount the dialog asks for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum AmountOperation {
    Expand,
    Contract,
}

impl AmountOperation {
    fn title(self) -> &'static str {
        match self {
            Self::Expand => "Expand Selection",
            Self::Contract => "Contract Selection",
        }
    }
}

/// The dialog's working copy; the amounts are remembered for the next time, as in
/// Compositor.
#[derive(Clone, Copy, Debug)]
pub(super) struct AmountEdit {
    pub operation: AmountOperation,
    pub amount: u32,
}

impl EditorApp {
    pub(super) fn open_selection_amount(&mut self, operation: AmountOperation) {
        if self
            .session()
            .is_none_or(|s| s.document.selection.is_none())
        {
            return;
        }
        let amount = match operation {
            AmountOperation::Expand => self.expand_amount,
            AmountOperation::Contract => self.contract_amount,
        };
        self.selection_amount = Some(AmountEdit { operation, amount });
        self.dialog = Some(Dialog::SelectionAmount);
    }

    /// Grows or shrinks the selection by `amount` pixels as one undo step, in the
    /// background so a large radius on a large image can be cancelled.
    pub(super) fn modify_selection(&mut self, operation: AmountOperation, amount: u32) {
        let amount = amount.clamp(1, selection_ops::MAX_AMOUNT);
        match operation {
            AmountOperation::Expand => self.expand_amount = amount,
            AmountOperation::Contract => self.contract_amount = amount,
        }
        self.start_job(tr(operation.title()), move |document, cancel| {
            let selection = document
                .selection
                .clone()
                .context(tr("Select an area first"))?;
            let result = match operation {
                AmountOperation::Expand => selection_ops::expand(&selection, amount, cancel),
                AmountOperation::Contract => selection_ops::contract(&selection, amount, cancel),
            }
            .context(tr("Cancelled"))?;
            document.selection = Some(Arc::new(result));
            Ok(())
        });
    }

    pub(super) fn selection_amount_dialog(&mut self, ctx: &egui::Context) {
        let Some(mut edit) = self.selection_amount else {
            self.dialog = None;
            return;
        };
        let mut open = true;
        let mut ok = false;
        let mut cancel = false;
        widgets::Window::new(tr(edit.operation.title()))
            .id("selection_amount")
            .open(&mut open)
            .default_width(280.0)
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.label(match edit.operation {
                        AmountOperation::Expand => tr("Expand by"),
                        AmountOperation::Contract => tr("Contract by"),
                    });
                    ui.add(
                        widgets::Number::new(&mut edit.amount)
                            .range(1..=selection_ops::MAX_AMOUNT)
                            .speed(1.0)
                            .suffix(" px"),
                    );
                });
                ui.add_space(14.0);
                ui.horizontal(|ui| {
                    cancel = widgets::button(ui, tr("Cancel")).clicked();
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ok = ui.add(widgets::Button::new(tr("OK")).primary()).clicked();
                    });
                });
            });
        let enter = ctx.input(|i| i.key_pressed(egui::Key::Enter)) && !ctx.wants_keyboard_input();
        if ok || enter {
            self.selection_amount = None;
            self.dialog = None;
            self.modify_selection(edit.operation, edit.amount);
        } else if cancel || !open || ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            self.selection_amount = None;
            self.dialog = None;
        } else {
            self.selection_amount = Some(edit);
        }
    }
}
