//! Following local imports: what `python main.py` really runs.

use std::fs;
use std::path::{Path, PathBuf};

use pyxray_core::model::{Effect, Severity};
use pyxray_core::{xray, xray_with, FollowOpts, XrayOpts};

/// A throwaway project directory.
struct Tree(PathBuf);

impl Tree {
    fn new(name: &str) -> Tree {
        let dir = std::env::temp_dir().join(format!(
            "pyxray-follow-{name}-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        Tree(dir)
    }

    fn file(&self, rel: &str, text: &str) -> &Tree {
        let path = self.0.join(rel);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
        self
    }

    fn run(&self, main: &str) -> pyxray_core::Report {
        let path = self.0.join(main);
        let source = fs::read_to_string(&path).unwrap();
        xray_with(
            &source,
            main,
            &XrayOpts {
                origin: Some(path),
                cwd: None,
                follow: FollowOpts::default(),
            },
        )
    }
}

impl Drop for Tree {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn linked_effects(r: &pyxray_core::Report) -> Vec<(String, Effect, String)> {
    r.linked
        .iter()
        .flat_map(|m| m.effects.iter())
        .map(|h| (h.file.clone().unwrap_or_default(), h.effect, h.verb.clone()))
        .collect()
}

#[test]
fn a_helper_that_shells_out_is_seen_through_the_driver() {
    let t = Tree::new("helper");
    t.file(
        "helper.py",
        "import os\n\ndef run(cmd):\n    os.system(cmd)\n",
    )
    .file("main.py", "import helper\nhelper.run('make clean')\n");
    let r = t.run("main.py");
    assert!(
        xray(&fs::read_to_string(t.0.join("main.py")).unwrap(), "m")
            .metrics
            .risk
            < 30
    );
    let hits = linked_effects(&r);
    assert!(
        hits.contains(&("helper.py".into(), Effect::Process, "shell".into())),
        "{hits:?}"
    );
    assert!(r.metrics.risk >= 30, "risk {}", r.metrics.risk);
    let event = r.event("test");
    assert!(event
        .notes
        .iter()
        .any(|n| n.file.as_deref() == Some("helper.py")));
    assert_eq!(r.worst(), Some(Severity::Caution));
}

#[test]
fn from_import_by_name_is_reached() {
    let t = Tree::new("fromimport");
    t.file(
        "tools.py",
        "import shutil\n\ndef wipe(p):\n    shutil.rmtree(p)\n",
    )
    .file("main.py", "from tools import wipe\nwipe('build')\n");
    let r = t.run("main.py");
    assert!(linked_effects(&r).iter().any(|h| h.1 == Effect::FsDelete));
}

#[test]
fn an_uncalled_function_is_counted_but_not_scored() {
    let t = Tree::new("uncalled");
    t.file(
        "utils.py",
        "import os\n\ndef used():\n    return 1\n\ndef dangerous():\n    os.system('rm -rf /')\n",
    )
    .file("main.py", "import utils\nprint(utils.used())\n");
    let r = t.run("main.py");
    assert!(linked_effects(&r).is_empty(), "{:?}", linked_effects(&r));
    assert_eq!(r.linked[0].unreached, 1);
    assert!(r.metrics.risk < 30);
}

#[test]
fn module_level_code_runs_on_import() {
    let t = Tree::new("toplevel");
    t.file(
        "setup_env.py",
        "import os\nos.environ['X'] = '1'\nos.remove('stale.lock')\n",
    )
    .file("main.py", "import setup_env\n");
    let hits = linked_effects(&t.run("main.py"));
    assert!(hits.iter().any(|h| h.1 == Effect::FsDelete), "{hits:?}");
}

#[test]
fn a_module_that_calls_its_own_main_at_the_bottom() {
    let t = Tree::new("ownmain");
    t.file(
        "job.py",
        "import subprocess\n\ndef main():\n    subprocess.run(['git', 'push'])\n\nmain()\n",
    )
    .file("main.py", "import job\n");
    let hits = linked_effects(&t.run("main.py"));
    assert!(hits.iter().any(|h| h.1 == Effect::Process), "{hits:?}");
}

#[test]
fn package_inits_run_and_submodules_resolve() {
    let t = Tree::new("package");
    t.file(
        "pkg/__init__.py",
        "import logging\nopen('pkg.log', 'a').write('init')\n",
    )
    .file(
        "pkg/net.py",
        "import requests\n\ndef fetch(u):\n    return requests.get(u)\n",
    )
    .file(
        "main.py",
        "from pkg import net\nnet.fetch('https://x.example')\n",
    );
    let r = t.run("main.py");
    let files: Vec<&str> = r.linked.iter().map(|m| m.file.as_str()).collect();
    assert!(
        files.contains(&"pkg/__init__.py") && files.contains(&"pkg/net.py"),
        "{files:?}"
    );
    let hits = linked_effects(&r);
    assert!(
        hits.iter()
            .any(|h| h.0 == "pkg/net.py" && h.1 == Effect::Net),
        "{hits:?}"
    );
    assert!(
        hits.iter()
            .any(|h| h.0 == "pkg/__init__.py" && h.1 == Effect::FsWrite),
        "{hits:?}"
    );
}

#[test]
fn relative_imports_inside_a_package() {
    let t = Tree::new("relative");
    t.file("app/__init__.py", "")
        .file(
            "app/db.py",
            "import sqlite3\n\ndef connect():\n    return sqlite3.connect('app.db')\n",
        )
        .file("app/models.py", "from . import db\nconn = db.connect()\n")
        .file("main.py", "import app.models\n");
    let hits = linked_effects(&t.run("main.py"));
    assert!(hits.iter().any(|h| h.0 == "app/db.py"), "{hits:?}");
}

#[test]
fn a_relative_import_in_the_script_itself_is_noted_not_followed() {
    let t = Tree::new("relscript");
    t.file("x.py", "import os\nos.system('ls')\n")
        .file("main.py", "from . import x\n");
    let r = t.run("main.py");
    assert!(r.linked.is_empty());
    let skipped = &r.follow.as_ref().unwrap().skipped;
    assert!(
        skipped
            .iter()
            .any(|(_, why)| why.contains("relative import")),
        "{skipped:?}"
    );
}

#[test]
fn import_cycles_terminate() {
    let t = Tree::new("cycle");
    t.file("a.py", "import b\nimport os\nos.getcwd()\n")
        .file("b.py", "import a\nopen('b.txt', 'w')\n")
        .file("main.py", "import a\n");
    let r = t.run("main.py");
    assert_eq!(
        r.linked.len(),
        2,
        "{:?}",
        r.linked.iter().map(|m| &m.file).collect::<Vec<_>>()
    );
}

#[cfg(unix)]
#[test]
fn a_symlink_out_of_the_directory_is_not_followed() {
    let outside = Tree::new("outside");
    outside.file("evil.py", "import os\nos.system('x')\n");
    let t = Tree::new("symlink");
    std::os::unix::fs::symlink(outside.0.join("evil.py"), t.0.join("evil.py")).unwrap();
    t.file("main.py", "import evil\n");
    let r = t.run("main.py");
    assert!(r.linked.is_empty());
    let skipped = &r.follow.as_ref().unwrap().skipped;
    assert!(skipped.iter().any(|(m, _)| m == "evil"), "{skipped:?}");
}

#[test]
fn the_file_limit_truncates() {
    let t = Tree::new("limit");
    let mut main = String::new();
    for i in 0..30 {
        t.file(&format!("m{i}.py"), "x = 1\n");
        main.push_str(&format!("import m{i}\n"));
    }
    t.file("main.py", &main);
    let r = t.run("main.py");
    assert_eq!(r.linked.len(), FollowOpts::default().max_files);
    assert!(r.follow.as_ref().unwrap().truncated);
}

#[test]
fn a_local_file_named_like_stdlib_is_not_followed() {
    let t = Tree::new("stdlib");
    t.file("json.py", "import os\nos.system('x')\n")
        .file("main.py", "import json\njson.dumps({})\n");
    assert!(t.run("main.py").linked.is_empty());
}

#[test]
fn installed_packages_are_not_the_snippet() {
    let t = Tree::new("installed");
    t.file(
        "main.py",
        "import requests\nrequests.get('https://x.example')\n",
    );
    let r = t.run("main.py");
    assert!(r.linked.is_empty() && r.follow.is_none());
}

#[test]
fn following_off_is_exactly_xray() {
    let t = Tree::new("off");
    t.file("helper.py", "import os\nos.system('x')\n")
        .file("main.py", "import helper\n");
    let path = t.0.join("main.py");
    let source = fs::read_to_string(&path).unwrap();
    let off = xray_with(
        &source,
        "main.py",
        &XrayOpts {
            origin: Some(path),
            cwd: None,
            follow: FollowOpts::off(),
        },
    );
    let plain = xray(&source, "main.py");
    assert_eq!(
        serde_json::to_string(&off).unwrap(),
        serde_json::to_string(&plain).unwrap()
    );
}

#[test]
fn a_snippet_without_a_file_resolves_from_cwd() {
    let t = Tree::new("cwd");
    t.file("helper.py", "import os\nos.remove('x')\n");
    let r = xray_with(
        "import helper\n",
        "-c",
        &XrayOpts {
            origin: None,
            cwd: Some(t.0.clone()),
            follow: FollowOpts::default(),
        },
    );
    assert!(linked_effects(&r).iter().any(|h| h.1 == Effect::FsDelete));
}

#[test]
fn nothing_to_resolve_from_means_nothing_followed() {
    let r = xray_with("import helper\n", "-c", &XrayOpts::default());
    assert!(r.linked.is_empty() && r.follow.is_none());
    let _ = Path::new(".");
}
