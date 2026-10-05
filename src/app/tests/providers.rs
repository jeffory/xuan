//! Providers: plugins standing in for Select Subject, Remove Background and the Magic
//! tool's Object mode.
use super::*;
use xuan::plugins::{Manifest, manifest::Capability};

const MANIFEST: &str = r#"
[plugin]
id = "seg"
name = "Seg"
version = "0.1.0"
command = ["sh", "plugin.sh"]

[permissions]
document = "edit"

[[provides]]
capability = "select_subject"
action = "segment"

[[provides]]
capability = "object_select"
action = "segment"

[[provides]]
capability = "remove_background"
action = "cutout"

[[actions]]
id = "segment"
label = "Segment…"
menu = "Select"
source = { from = "composite" }

[[actions.inputs]]
id = "detail"
type = "integer"
default = 3

[[actions]]
id = "cutout"
label = "Cut Out"
source = { from = "layer" }
"#;

/// A 40x30 document: a white square (10..30, 5..25) on black, so the mock's mask (its
/// source, read as luminance) is exactly the square.
fn square_document(app: &mut EditorApp) {
    app.dimensions = [40, 30];
    app.new_document();
    let pixels = RgbaImage::from_fn(40, 30, |x, y| {
        let inside = (10..30).contains(&x) && (5..25).contains(&y);
        image::Rgba(if inside {
            [255, 255, 255, 255]
        } else {
            [0, 0, 0, 255]
        })
    });
    app.session_mut().unwrap().document.layers[0].pixels = Some(Arc::new(pixels));
}

fn in_square(i: usize) -> bool {
    let (x, y) = (i % 40, i / 40);
    (10..30).contains(&x) && (5..25).contains(&y)
}

#[test]
fn manifests_declare_providers_and_remove_background_needs_edit() {
    let dir = tempfile::tempdir().unwrap();
    let manifest = Manifest::parse(MANIFEST, dir.path()).unwrap();
    assert_eq!(
        manifest.provider(Capability::SelectSubject).unwrap().id,
        "segment"
    );
    assert_eq!(
        manifest.provider(Capability::RemoveBackground).unwrap().id,
        "cutout"
    );
    let error = |text: &str| {
        format!(
            "{:#}",
            Manifest::parse(text, dir.path()).expect_err("should be refused")
        )
    };
    let read = MANIFEST.replace("document = \"edit\"", "document = \"read\"");
    assert!(
        error(&read).contains("remove_background"),
        "{}",
        error(&read)
    );
    let missing = MANIFEST.replace("action = \"cutout\"", "action = \"nothing\"");
    assert!(error(&missing).contains("nothing"));
    let twice = MANIFEST.replace(
        "capability = \"object_select\"",
        "capability = \"select_subject\"",
    );
    assert!(error(&twice).contains("twice"));
    let unsourced = MANIFEST.replace(
        "source = { from = \"layer\" }",
        "source = { from = \"none\" }",
    );
    assert!(error(&unsourced).contains("source image"));
    // A read-only plugin may provide the selection capabilities.
    let read_ok = read.replace(
        "[[provides]]\ncapability = \"remove_background\"\naction = \"cutout\"\n",
        "",
    );
    assert!(Manifest::parse(&read_ok, dir.path()).is_ok());
}

#[test]
fn the_built_in_runs_without_a_provider_and_a_missing_one_falls_back_with_a_notice() {
    let (_, mut app) = app();
    square_document(&mut app);
    assert_eq!(app.config.providers, xuan::config::Providers::default());
    app.command("select_subject");
    assert_eq!(
        app.job.as_ref().map(|j| j.name.as_str()),
        Some("Select Subject")
    );
    super::selection::wait_for_job(&mut app);
    let selection = app.session().unwrap().document.selection.clone().unwrap();
    assert!(
        selection
            .as_raw()
            .iter()
            .enumerate()
            .all(|(i, &v)| (v >= 128) == in_square(i))
    );

    // A chosen plugin that is not installed: the built-in runs, with a notice.
    app.config
        .providers
        .set(Capability::SelectSubject, Some("gone".into()));
    app.command("select_subject");
    assert!(app.job.is_some());
    super::selection::wait_for_job(&mut app);
    assert!(app.status.contains("gone"), "{}", app.status);
    assert!(app.status.contains("built-in"), "{}", app.status);
}

#[test]
fn settings_list_the_plugins_that_provide_each_capability() {
    let dir = tempfile::tempdir().unwrap();
    let (_, mut app) = app();
    std::fs::write(dir.path().join("plugin.toml"), MANIFEST).unwrap();
    let manifest = Manifest::load(dir.path()).unwrap();
    app.install_plugins(vec![manifest], vec![]);
    let choices = app.provider_choices(Capability::RemoveBackground);
    assert_eq!(
        choices,
        [
            (None, "Built-in".to_owned()),
            (Some("seg".to_owned()), "Seg (seg)".to_owned())
        ]
    );
}

#[cfg(unix)]
mod unix {
    use super::*;
    use std::time::{Duration, Instant};

    fn run_until(context: &egui::Context, app: &mut EditorApp, done: impl Fn(&EditorApp) -> bool) {
        let deadline = Instant::now() + Duration::from_secs(20);
        while !done(app) {
            assert!(
                Instant::now() < deadline,
                "timed out; error: {:?}",
                app.error
            );
            frame(context, app);
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    fn install_seg(app: &mut EditorApp, dir: &Path, manifest: &str) {
        std::fs::write(dir.join("plugin.toml"), manifest).unwrap();
        // Every request is logged; action/run answers with its source as the mask.
        std::fs::write(
            dir.join("plugin.sh"),
            r#"
    while IFS= read -r line; do
      printf '%s\n' "$line" >> received.log
      id=$(printf '%s' "$line" | sed -n 's/.*"id":\([0-9][0-9]*\),"method".*/\1/p')
      case "$line" in
        *'"method":"initialize"'*)
          printf '{"jsonrpc":"2.0","id":%s,"result":{"protocol":1}}\n' "$id" ;;
        *'"method":"action/run"'*)
          work=$(printf '%s' "$line" | sed -n 's/.*"work_dir":"\([^"]*\)".*/\1/p')
          src=$(printf '%s' "$line" | sed -n 's/.*"source":{.*"path":"\([^"]*\)".*/\1/p')
          cp "$src" "$work/mask.png"
          printf '{"jsonrpc":"2.0","id":%s,"result":{"outputs":[{"kind":"mask","path":"%s/mask.png","fit":"source"}]}}\n' "$id" "$work" ;;
        *'"method":"shutdown"'*)
          printf '{"jsonrpc":"2.0","id":%s,"result":null}\n' "$id"; exit 0 ;;
        *) ;;
      esac
    done
    "#,
        )
        .unwrap();
        let manifest = Manifest::load(dir).unwrap();
        app.install_plugins(vec![manifest], vec![]);
        app.grant_plugin("seg", true);
    }

    fn received(dir: &Path) -> String {
        std::fs::read_to_string(dir.join("received.log")).unwrap_or_default()
    }

    #[test]
    fn a_chosen_provider_runs_without_its_dialog_and_proposes_the_selection() {
        let dir = tempfile::tempdir().unwrap();
        let (context, mut app) = app();
        install_seg(&mut app, dir.path(), MANIFEST);
        square_document(&mut app);
        app.config
            .providers
            .set(Capability::SelectSubject, Some("seg".into()));
        app.command("select_subject");
        // No built-in job and no action dialog: the plugin job runs at once.
        assert!(app.job.is_none());
        assert!(app.plugins.action.is_none());
        assert_eq!(app.plugins.jobs.len(), 1, "{:?}", app.error);
        run_until(&context, &mut app, |app| {
            app.dialog == Some(Dialog::PluginProposal)
        });
        let log = received(dir.path());
        assert!(log.contains(r#""capability":"select_subject""#), "{log}");
        assert!(log.contains(r#""detail":3"#), "{log}");
        app.resolve_proposal(true);
        let session = app.session().unwrap();
        let selection = session.document.selection.clone().unwrap();
        assert!(
            selection
                .as_raw()
                .iter()
                .enumerate()
                .all(|(i, &v)| (v >= 128) == in_square(i))
        );
        assert_eq!(session.history.undo_name(), Some("Select Subject"));
    }

    #[test]
    fn object_mode_sends_the_click_and_combines_as_asked() {
        let dir = tempfile::tempdir().unwrap();
        let (context, mut app) = app();
        install_seg(&mut app, dir.path(), MANIFEST);
        square_document(&mut app);
        app.config
            .providers
            .set(Capability::ObjectSelect, Some("seg".into()));
        // Something already selected outside the square; Shift adds to it.
        app.edit_selection("Corner", |doc| {
            let mut mask = GrayImage::new(40, 30);
            mask.put_pixel(0, 0, image::Luma([255]));
            doc.selection = Some(Arc::new(mask));
        });
        app.select_object(
            xuan::segment::Seeds {
                points: vec![(20.5, 15.5)],
                rect: None,
            },
            xuan::selection::SelectionMode::Add,
        );
        run_until(&context, &mut app, |app| {
            app.dialog == Some(Dialog::PluginProposal)
        });
        let log = received(dir.path());
        assert!(log.contains(r#""capability":"object_select""#), "{log}");
        assert!(log.contains(r#""point":{"x":20.5,"y":15.5}"#), "{log}");
        app.resolve_proposal(true);
        let selection = app.session().unwrap().document.selection.clone().unwrap();
        assert_eq!(selection.get_pixel(0, 0)[0], 255);
        assert_eq!(selection.get_pixel(20, 15)[0], 255);
        assert_eq!(selection.get_pixel(35, 15)[0], 0);
    }

    #[test]
    fn remove_background_through_a_provider_masks_the_layer() {
        let dir = tempfile::tempdir().unwrap();
        let (context, mut app) = app();
        install_seg(&mut app, dir.path(), MANIFEST);
        square_document(&mut app);
        app.config
            .providers
            .set(Capability::RemoveBackground, Some("seg".into()));
        app.command("remove_background");
        run_until(&context, &mut app, |app| {
            app.dialog == Some(Dialog::PluginProposal)
        });
        let document = &app.session().unwrap().document;
        assert!(document.selection.is_none());
        let image = document.layers[0].id;
        let mask = document
            .layers
            .iter()
            .find(|l| l.parent == Some(image))
            .unwrap();
        let pixels = &mask.mask.as_ref().unwrap().pixels;
        assert!(
            pixels
                .as_raw()
                .iter()
                .enumerate()
                .all(|(i, &v)| (v >= 128) == in_square(i))
        );
        // Compare hides the new mask; Discard takes it away again.
        app.toggle_proposal_compare();
        let document = &app.session().unwrap().document;
        assert!(
            !document
                .layers
                .iter()
                .find(|l| l.parent == Some(image))
                .unwrap()
                .visible
        );
        app.toggle_proposal_compare();
        app.resolve_proposal(false);
        assert_eq!(app.session().unwrap().document.layers.len(), 1);
    }

    #[test]
    fn a_disabled_or_offline_provider_falls_back_to_the_built_in() {
        let dir = tempfile::tempdir().unwrap();
        let (_, mut app) = app();
        // Declares a host, so offline mode turns it off.
        let networked = MANIFEST.replace(
            "document = \"edit\"",
            "document = \"edit\"\nnetwork = [\"example.com\"]",
        );
        install_seg(&mut app, dir.path(), &networked);
        square_document(&mut app);
        app.config
            .providers
            .set(Capability::SelectSubject, Some("seg".into()));
        app.config.disable_network_plugins = true;
        app.command("select_subject");
        assert!(app.plugins.jobs.is_empty());
        assert_eq!(
            app.job.as_ref().map(|j| j.name.as_str()),
            Some("Select Subject")
        );
        super::super::selection::wait_for_job(&mut app);
        assert!(app.status.contains("Seg (plugin seg)"), "{}", app.status);
        assert!(app.status.contains("network"), "{}", app.status);

        app.config.disable_network_plugins = false;
        app.config.plugins.entry("seg".into()).or_default().enabled = false;
        app.command("select_subject");
        assert!(app.plugins.jobs.is_empty());
        super::super::selection::wait_for_job(&mut app);
        assert!(app.status.contains("disabled"), "{}", app.status);
        // Nothing was sent to the plugin.
        assert!(!received(dir.path()).contains("action/run"));
    }

    #[test]
    fn a_provider_waiting_for_its_grant_continues_as_the_provider() {
        let dir = tempfile::tempdir().unwrap();
        let (context, mut app) = app();
        install_seg(&mut app, dir.path(), MANIFEST);
        app.grant_plugin("seg", false);
        square_document(&mut app);
        app.config
            .providers
            .set(Capability::SelectSubject, Some("seg".into()));
        app.command("select_subject");
        assert_eq!(app.dialog, Some(Dialog::PluginPermissions));
        assert!(app.job.is_none());
        app.grant_plugin("seg", true);
        app.dialog = None;
        let (plugin, _) = app.plugins.permission_request.take().unwrap();
        app.start_plugin_action_with(&plugin, "segment", None);
        assert!(app.plugins.action.is_none(), "no dialog for a provider run");
        run_until(&context, &mut app, |app| {
            app.dialog == Some(Dialog::PluginProposal)
        });
        assert_eq!(
            app.plugins.proposal.as_ref().unwrap().name,
            "Select Subject"
        );
        app.resolve_proposal(false);
        // Running the action from its menu afterwards is an ordinary run with a dialog.
        app.start_plugin_action("seg", "segment");
        assert!(app.plugins.action.is_some());
    }
}
