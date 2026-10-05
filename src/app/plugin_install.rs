//! Plugins → Install from Folder or Zip…: pick or drop a plugin, review what
//! it is and asks for, then copy it into the plugins directory.
//!
//! Installing is not allowing. The review shows the same facts as the
//! permission prompt, but the plugin is allowed only through that prompt, the
//! first time it starts, where it shows the installed folder. An update keeps
//! the grant only when the folder, command and permissions are unchanged, as
//! any grant does, and the review says which applies.
use std::path::{Path, PathBuf};

use egui::RichText;
use xuan::{
    i18n::tr,
    plugins::{
        self,
        install::{Installer, Staged},
        sandbox,
    },
};

use super::{
    Dialog, EditorApp,
    plugins::{grant_for, one_line},
    theme, widgets,
};

/// The install window: choosing a source, or reviewing a staged plugin.
#[derive(Default)]
pub(super) struct PluginInstall {
    pub staged: Option<Staged>,
    pub error: Option<String>,
    /// Go back to Manage Plugins when the window closes.
    pub from_manager: bool,
}

/// What happens to the plugin's permission once it is installed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum GrantAfterInstall {
    /// Nothing allowed yet: it asks the first time it starts.
    Asks,
    /// Allowed before with the same folder, command and permissions.
    Kept,
    /// Allowed before, but the folder, command or permissions differ.
    AsksAgain,
}

/// Whether a dropped path is something to install: a folder or a `.zip`.
pub(super) fn installable(path: &Path) -> bool {
    path.is_dir()
        || path
            .extension()
            .is_some_and(|extension| extension.eq_ignore_ascii_case("zip"))
}

impl EditorApp {
    /// The installer for the user plugins directory, if there is a
    /// configuration folder to hold it.
    pub(super) fn plugin_installer(&self) -> Option<Installer> {
        let config_dir = self.config_path.as_ref()?.parent()?;
        let search_dirs = plugins::plugin_dirs(
            Some(config_dir),
            std::env::var_os(plugins::PATH_VARIABLE).as_deref(),
        );
        Some(Installer::new(config_dir.join("plugins"), search_dirs))
    }

    /// Open the install window to choose a folder or an archive.
    pub(super) fn open_plugin_install(&mut self) {
        let from_manager = self.dialog == Some(Dialog::Plugins);
        self.plugins.install = Some(PluginInstall {
            from_manager,
            ..PluginInstall::default()
        });
        self.dialog = Some(Dialog::PluginInstall);
    }

    /// Copy `source` into a private staging folder and check it, then show
    /// the review, or the reason it cannot be installed.
    pub(super) fn stage_plugin_install(&mut self, source: &Path) {
        let from_manager = self
            .plugins
            .install
            .as_ref()
            .map_or(self.dialog == Some(Dialog::Plugins), |i| i.from_manager);
        let result = match self.plugin_installer() {
            Some(installer) => installer.prepare(source),
            None => Err(anyhow::anyhow!(tr(
                "There is no configuration folder to install plugins into."
            ))),
        };
        let (staged, error) = match result {
            Ok(staged) => (Some(staged), None),
            Err(error) => (None, Some(format!("{error:#}"))),
        };
        self.plugins.install = Some(PluginInstall {
            staged,
            error,
            from_manager,
        });
        self.dialog = Some(Dialog::PluginInstall);
    }

    /// What installing the staged plugin does to its permission.
    pub(super) fn grant_after_install(&self, staged: &Staged) -> GrantAfterInstall {
        let mut installed = staged.manifest.clone();
        installed.dir = staged.target.clone();
        match self.stored_grant(&staged.manifest.plugin.id) {
            None => GrantAfterInstall::Asks,
            Some(grant) if grant.covers(&grant_for(&installed)) => GrantAfterInstall::Kept,
            Some(_) => GrantAfterInstall::AsksAgain,
        }
    }

    /// Install the reviewed plugin, reload the plugins and show it in
    /// Manage Plugins.
    pub(super) fn commit_plugin_install(&mut self) {
        let Some(staged) = self.plugins.install.as_mut().and_then(|i| i.staged.take()) else {
            return;
        };
        let Some(installer) = self.plugin_installer() else {
            return;
        };
        let id = staged.manifest.plugin.id.clone();
        let source = plugin_source(&staged);
        // The running process holds files in the folder about to be replaced.
        if staged.update && self.plugins.manifest(&id).is_some() {
            self.end_plugin(&id, &format!("{source} {}", tr("is being updated")));
        }
        match installer.commit(staged) {
            Ok(_) => {
                self.load_plugins();
                self.status = format!("{} {source}", tr("Installed"));
                self.plugins.install = None;
                self.plugins.manager_selected = Some(id);
                self.dialog = Some(Dialog::Plugins);
            }
            Err(error) => {
                if let Some(install) = &mut self.plugins.install {
                    install.error = Some(format!("{error:#}"));
                }
            }
        }
    }

    fn close_plugin_install(&mut self) {
        let from_manager = self.plugins.install.take().is_some_and(|i| i.from_manager);
        self.dialog = from_manager.then_some(Dialog::Plugins);
    }

    pub(super) fn plugin_install_dialog(&mut self, ctx: &egui::Context) {
        let Some(install) = &self.plugins.install else {
            self.dialog = None;
            return;
        };
        let mut open = true;
        let mut pick: Option<PathBuf> = None;
        let mut confirm = false;
        let mut cancel = false;
        let error = install.error.clone();
        match &install.staged {
            None => {
                widgets::Window::new(tr("Install Plugin"))
                    .id("plugin_install")
                    .default_width(420.0)
                    .open(&mut open)
                    .show(ctx, |ui| {
                        if let Some(error) = &error {
                            ui.label(RichText::new(tr("Cannot install this plugin")).strong());
                            ui.add(
                                egui::Label::new(
                                    RichText::new(error).color(ui.visuals().error_fg_color),
                                )
                                .wrap(),
                            );
                            ui.add_space(8.0);
                        }
                        ui.add(
                            egui::Label::new(tr(
                                "Choose a plugin folder or a .zip archive. Xuan checks it and shows what it asks for before anything is copied into your plugins folder.",
                            ))
                            .wrap(),
                        );
                        ui.add_space(4.0);
                        ui.label(
                            RichText::new(tr("You can also drop a folder or a .zip file here."))
                                .small()
                                .color(theme::MUTED),
                        );
                        ui.add_space(12.0);
                        ui.separator();
                        ui.horizontal(|ui| {
                            if widgets::button(ui, tr("Choose Zip…")).clicked() {
                                pick = rfd::FileDialog::new()
                                    .add_filter(tr("Zip archive"), &["zip"])
                                    .pick_file();
                            }
                            if widgets::button(ui, tr("Choose Folder…")).clicked() {
                                pick = rfd::FileDialog::new().pick_folder();
                            }
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    cancel = widgets::button(ui, tr("Cancel")).clicked();
                                },
                            );
                        });
                    });
            }
            Some(staged) => {
                let manifest = &staged.manifest;
                let source = plugin_source(staged);
                let grant = self.grant_after_install(staged);
                let blocked = sandbox::blocks_network(
                    self.config.block_undeclared_network(),
                    &manifest.permissions,
                );
                let title = format!(
                    "{} {source}?",
                    if staged.update {
                        tr("Update")
                    } else {
                        tr("Install")
                    }
                );
                widgets::Window::new(title)
                    .id("plugin_install")
                    .default_width(440.0)
                    .open(&mut open)
                    .show(ctx, |ui| {
                        ui.add(
                            egui::Label::new(format!(
                                "{source} {} {}",
                                manifest.plugin.version,
                                if blocked {
                                    tr("runs as a program with your rights. Xuan enforces the permissions below only for what Xuan itself sends the plugin and does for it, and blocks its network. The plugin can still read your files. Only run plugins you trust.")
                                } else {
                                    tr("runs as a program with your rights. Xuan enforces the permissions below only for what Xuan itself sends the plugin and does for it. The plugin can still read your files and contact any server, whatever it declares. Only run plugins you trust.")
                                },
                            ))
                            .wrap(),
                        );
                        ui.add_space(8.0);
                        let facts = [
                            (tr("Name:"), one_line(&manifest.plugin.name, 80)),
                            (tr("Id:"), manifest.plugin.id.clone()),
                            (tr("Version:"), one_line(&manifest.plugin.version, 40)),
                            (tr("From:"), staged.source.display().to_string()),
                            (tr("Installs to:"), staged.target.display().to_string()),
                            (tr("Runs:"), manifest.plugin.command.join(" ")),
                        ];
                        for (label, value) in facts {
                            ui.add(
                                egui::Label::new(
                                    RichText::new(format!("{label} {value}"))
                                        .small()
                                        .color(theme::MUTED),
                                )
                                .wrap(),
                            );
                        }
                        ui.add_space(8.0);
                        ui.label(RichText::new(tr("Permissions")).strong());
                        super::plugin_dialogs::permissions_list(ui, manifest, blocked);
                        ui.add_space(8.0);
                        if staged.update {
                            let replaces = match &staged.previous {
                                Some(previous) => format!(
                                    "{} {}",
                                    tr("Replaces the installed version"),
                                    one_line(&previous.plugin.version, 40)
                                ),
                                None => tr("Replaces the plugin installed in this folder").to_owned(),
                            };
                            ui.label(format!("• {replaces}"));
                        }
                        let note = match grant {
                            GrantAfterInstall::Asks => tr("Installing does not allow it to run: Xuan asks for that the first time it starts."),
                            GrantAfterInstall::Kept => tr("It stays allowed: you allowed it before with the same folder, command and permissions."),
                            GrantAfterInstall::AsksAgain => tr("Its folder, command or permissions differ from what you allowed, so Xuan asks again before it runs."),
                        };
                        ui.add(egui::Label::new(format!("• {note}")).wrap());
                        if let Some(error) = &error {
                            ui.add_space(8.0);
                            ui.add(
                                egui::Label::new(
                                    RichText::new(error).color(ui.visuals().error_fg_color),
                                )
                                .wrap(),
                            );
                        }
                        ui.add_space(12.0);
                        ui.separator();
                        ui.horizontal(|ui| {
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    let label = if staged.update {
                                        tr("Update")
                                    } else {
                                        tr("Install")
                                    };
                                    confirm = widgets::primary_button(ui, label).clicked();
                                    cancel = widgets::button(ui, tr("Cancel")).clicked();
                                },
                            );
                        });
                    });
            }
        }
        if let Some(path) = pick {
            self.stage_plugin_install(&path);
        } else if confirm {
            self.commit_plugin_install();
        } else if cancel || !open {
            self.close_plugin_install();
        }
    }
}

/// "Name (plugin id)", as the permission prompt names a plugin.
fn plugin_source(staged: &Staged) -> String {
    format!(
        "{} ({} {})",
        one_line(&staged.manifest.plugin.name, 80),
        tr("plugin"),
        staged.manifest.plugin.id
    )
}
