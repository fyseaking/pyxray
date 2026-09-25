//! The catalogue: which Python symbols mean which real-world side effect.
//!
//! Everything here is a static table keyed by *canonical* dotted path — the
//! path after import aliases have been unwound, so `np.load` and
//! `numpy.load` both arrive as `numpy.load`. Method calls on values we have
//! inferred a kind for (`f = open(p)` … `f.write(x)`) arrive as
//! `<file>.write`, which keeps them in the same namespace as everything else.

use std::collections::HashMap;
use std::sync::OnceLock;

use crate::model::{Effect, Severity, ValueKind};

/// Where in the call's arguments to look for the thing being acted upon.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TargetArg {
    None,
    /// Positional index.
    Pos(usize),
    /// Keyword name, falling back to a positional index.
    Named(&'static str, usize),
}

#[derive(Clone, Copy, Debug)]
pub struct Rule {
    pub path: &'static str,
    pub effect: Effect,
    pub sev: Severity,
    pub verb: &'static str,
    pub target: TargetArg,
    pub note: Option<&'static str>,
}

const fn r(
    path: &'static str,
    effect: Effect,
    sev: Severity,
    verb: &'static str,
    target: TargetArg,
) -> Rule {
    Rule {
        path,
        effect,
        sev,
        verb,
        target,
        note: None,
    }
}

const fn rn(
    path: &'static str,
    effect: Effect,
    sev: Severity,
    verb: &'static str,
    target: TargetArg,
    note: &'static str,
) -> Rule {
    Rule {
        path,
        effect,
        sev,
        verb,
        target,
        note: Some(note),
    }
}

use Effect::*;
use Severity::{Caution, Info, Notable};
use TargetArg::{Named, None as NoArg, Pos};

#[rustfmt::skip]
pub const RULES: &[Rule] = &[
    // ---- filesystem: read -------------------------------------------------
    r("os.listdir",              FsRead, Info,    "list",   Pos(0)),
    r("os.scandir",              FsRead, Info,    "list",   Pos(0)),
    r("os.walk",                 FsRead, Info,    "walk",   Pos(0)),
    r("os.stat",                 FsRead, Info,    "stat",   Pos(0)),
    r("os.path.exists",          FsRead, Info,    "test",   Pos(0)),
    r("os.path.isfile",          FsRead, Info,    "test",   Pos(0)),
    r("os.path.isdir",           FsRead, Info,    "test",   Pos(0)),
    r("os.path.getsize",         FsRead, Info,    "stat",   Pos(0)),
    r("glob.glob",               FsRead, Info,    "glob",   Pos(0)),
    r("glob.iglob",              FsRead, Info,    "glob",   Pos(0)),
    r("json.load",               FsRead, Info,    "parse",  Pos(0)),
    r("tomllib.load",            FsRead, Info,    "parse",  Pos(0)),
    r("csv.reader",              FsRead, Info,    "parse",  Pos(0)),
    r("csv.DictReader",          FsRead, Info,    "parse",  Pos(0)),
    r("yaml.safe_load",          FsRead, Info,    "parse",  Pos(0)),
    r("pandas.read_csv",         FsRead, Info,    "read",   Named("filepath_or_buffer", 0)),
    r("pandas.read_parquet",     FsRead, Info,    "read",   Pos(0)),
    r("pandas.read_json",        FsRead, Info,    "read",   Pos(0)),
    r("pandas.read_excel",       FsRead, Info,    "read",   Pos(0)),
    r("pandas.read_hdf",         FsRead, Info,    "read",   Pos(0)),
    r("pandas.read_table",       FsRead, Info,    "read",   Pos(0)),
    r("pandas.read_sql",         FsRead, Info,    "query",  Pos(0)),
    r("numpy.load",              FsRead, Info,    "read",   Pos(0)),
    r("numpy.loadtxt",           FsRead, Info,    "read",   Pos(0)),
    r("numpy.genfromtxt",        FsRead, Info,    "read",   Pos(0)),
    r("numpy.fromfile",          FsRead, Info,    "read",   Pos(0)),
    r("cv2.imread",              FsRead, Info,    "read",   Pos(0)),
    r("PIL.Image.open",          FsRead, Info,    "read",   Pos(0)),
    r("nibabel.load",            FsRead, Info,    "read",   Pos(0)),
    r("h5py.File",               FsRead, Info,    "open",   Pos(0)),
    r("<path>.read_text",        FsRead, Info,    "read",   NoArg),
    r("<path>.read_bytes",       FsRead, Info,    "read",   NoArg),
    r("<path>.iterdir",          FsRead, Info,    "list",   NoArg),
    r("<path>.glob",             FsRead, Info,    "glob",   Pos(0)),
    r("<path>.rglob",            FsRead, Info,    "glob",   Pos(0)),
    r("<path>.exists",           FsRead, Info,    "test",   NoArg),
    r("<path>.stat",             FsRead, Info,    "stat",   NoArg),
    r("<path>.open",             FsRead, Info,    "open",   NoArg),
    r("<path>.is_file",          FsRead, Info,    "test",   NoArg),
    r("<path>.is_dir",           FsRead, Info,    "test",   NoArg),
    r("<path>.resolve",          FsRead, Info,    "stat",   NoArg),
    r("os.path.expanduser",      FsRead, Info,    "expand", Pos(0)),
    r("shelve.open",             FsRead, Notable, "open",   Pos(0)),
    r("<response>.json",         Net, Info,    "body",   NoArg),
    r("<response>.text",         Net, Info,    "body",   NoArg),
    r("<response>.read",         Net, Info,    "body",   NoArg),
    r("<response>.iter_content", Net, Info,    "body",   NoArg),
    r("<file>.read",             FsRead, Info,    "read",   NoArg),
    r("<file>.readlines",        FsRead, Info,    "read",   NoArg),
    r("<file>.readline",         FsRead, Info,    "read",   NoArg),

    // ---- filesystem: write ------------------------------------------------
    r("os.makedirs",             FsWrite, Info,    "mkdir",  Pos(0)),
    r("os.mkdir",                FsWrite, Info,    "mkdir",  Pos(0)),
    r("os.rename",               FsWrite, Notable, "rename", Pos(0)),
    r("os.replace",              FsWrite, Notable, "replace", Pos(1)),
    r("os.symlink",              FsWrite, Notable, "link",   Pos(1)),
    r("os.link",                 FsWrite, Notable, "link",   Pos(1)),
    r("os.chmod",                FsWrite, Notable, "chmod",  Pos(0)),
    r("os.chown",                FsWrite, Notable, "chown",  Pos(0)),
    r("os.truncate",             FsWrite, Notable, "truncate", Pos(0)),
    r("shutil.copy",             FsWrite, Notable, "copy",   Pos(1)),
    r("shutil.copy2",            FsWrite, Notable, "copy",   Pos(1)),
    r("shutil.copyfile",         FsWrite, Notable, "copy",   Pos(1)),
    r("shutil.copytree",         FsWrite, Notable, "copy",   Pos(1)),
    r("shutil.move",             FsWrite, Notable, "move",   Pos(1)),
    r("shutil.make_archive",     FsWrite, Notable, "archive", Pos(0)),
    r("shutil.unpack_archive",   FsWrite, Notable, "unpack", Pos(1)),
    r("json.dump",               FsWrite, Notable, "write",  Pos(1)),
    r("yaml.dump",               FsWrite, Notable, "write",  Pos(1)),
    r("pickle.dump",             FsWrite, Notable, "write",  Pos(1)),
    r("numpy.save",              FsWrite, Notable, "write",  Pos(0)),
    r("numpy.savez",             FsWrite, Notable, "write",  Pos(0)),
    r("numpy.savez_compressed",  FsWrite, Notable, "write",  Pos(0)),
    r("numpy.savetxt",           FsWrite, Notable, "write",  Pos(0)),
    r("torch.save",              FsWrite, Notable, "write",  Pos(1)),
    r("matplotlib.pyplot.savefig", FsWrite, Notable, "plot", Pos(0)),
    r("cv2.imwrite",             FsWrite, Notable, "write",  Pos(0)),
    r("tempfile.NamedTemporaryFile", FsWrite, Info, "tmpfile", NoArg),
    r("tempfile.mkstemp",        FsWrite, Info,    "tmpfile", NoArg),
    r("tempfile.mkdtemp",        FsWrite, Info,    "tmpdir",  NoArg),
    r("tempfile.TemporaryDirectory", FsWrite, Info, "tmpdir", NoArg),
    r("<path>.write_text",       FsWrite, Notable, "write",  NoArg),
    r("<path>.write_bytes",      FsWrite, Notable, "write",  NoArg),
    r("<path>.mkdir",            FsWrite, Info,    "mkdir",  NoArg),
    r("<path>.touch",            FsWrite, Info,    "touch",  NoArg),
    r("<path>.rename",           FsWrite, Notable, "rename", Pos(0)),
    r("<path>.replace",          FsWrite, Notable, "replace", Pos(0)),
    r("<path>.chmod",            FsWrite, Notable, "chmod",  NoArg),
    r("<path>.symlink_to",       FsWrite, Notable, "link",   Pos(0)),
    r("<path>.hardlink_to",      FsWrite, Notable, "link",   Pos(0)),
    rn("<archive>.extractall",   FsWrite, Notable, "unpack", NoArg, "extracts an archive; member paths decide where"),
    rn("<archive>.extract",      FsWrite, Notable, "unpack", Pos(0), "extracts an archive; member paths decide where"),
    r("zipfile.ZipFile",         FsRead,  Info,    "open",   Pos(0)),
    r("tarfile.open",            FsRead,  Info,    "open",   Pos(0)),
    r("tarfile.TarFile",         FsRead,  Info,    "open",   Pos(0)),
    rn("<conn>.execute",         FsWrite, Notable, "sql",    Pos(0), "runs a database statement"),
    rn("<conn>.executemany",     FsWrite, Notable, "sql",    Pos(0), "runs a database statement"),
    rn("<conn>.executescript",   FsWrite, Notable, "sql",    Pos(0), "runs a database script"),
    r("<conn>.commit",           FsWrite, Info,    "commit", NoArg),
    r("sqlite3.connect",         FsRead,  Info,    "open",   Pos(0)),
    r("<file>.write",            FsWrite, Notable, "write",  NoArg),
    r("<file>.writelines",       FsWrite, Notable, "write",  NoArg),
    r("<frame>.to_csv",          FsWrite, Notable, "write",  Pos(0)),
    r("<frame>.to_parquet",      FsWrite, Notable, "write",  Pos(0)),
    r("<frame>.to_json",         FsWrite, Notable, "write",  Pos(0)),
    r("<frame>.to_excel",        FsWrite, Notable, "write",  Pos(0)),
    r("<frame>.to_pickle",       FsWrite, Notable, "write",  Pos(0)),
    r("<frame>.to_sql",          FsWrite, Notable, "write",  Pos(0)),

    // ---- filesystem: destroy ----------------------------------------------
    rn("os.remove",       FsDelete, Caution, "delete", Pos(0), "removes a file from disk"),
    rn("os.unlink",       FsDelete, Caution, "delete", Pos(0), "removes a file from disk"),
    rn("os.rmdir",        FsDelete, Caution, "rmdir",  Pos(0), "removes a directory"),
    rn("os.removedirs",   FsDelete, Caution, "rmdir",  Pos(0), "removes a directory tree"),
    rn("shutil.rmtree",   FsDelete, Caution, "rmtree", Pos(0), "recursively deletes a directory"),
    rn("<path>.unlink",   FsDelete, Caution, "delete", NoArg,  "removes a file from disk"),
    rn("<path>.rmdir",    FsDelete, Caution, "rmdir",  NoArg,  "removes a directory"),
    rn("os.rmtree",       FsDelete, Caution, "rmtree", Pos(0), "recursively deletes a directory"),
    rn("send2trash.send2trash", FsDelete, Notable, "trash", Pos(0), "moves a path to the trash"),

    // ---- network ----------------------------------------------------------
    r("requests.get",            Net, Notable, "GET",    Pos(0)),
    r("requests.head",           Net, Info,    "HEAD",   Pos(0)),
    rn("requests.post",          Net, Caution, "POST",   Pos(0), "sends data to a remote host"),
    rn("requests.put",           Net, Caution, "PUT",    Pos(0), "sends data to a remote host"),
    rn("requests.patch",         Net, Caution, "PATCH",  Pos(0), "sends data to a remote host"),
    rn("requests.delete",        Net, Caution, "DELETE", Pos(0), "deletes a remote resource"),
    r("requests.request",        Net, Notable, "request", Pos(1)),
    r("requests.Session",        Net, Notable, "session", NoArg),
    r("httpx.get",               Net, Notable, "GET",    Pos(0)),
    rn("httpx.post",             Net, Caution, "POST",   Pos(0), "sends data to a remote host"),
    r("httpx.Client",            Net, Notable, "session", NoArg),
    r("httpx.AsyncClient",       Net, Notable, "session", NoArg),
    r("aiohttp.ClientSession",   Net, Notable, "session", NoArg),
    r("urllib.request.urlopen",  Net, Notable, "fetch",  Pos(0)),
    rn("urllib.request.urlretrieve", Net, Caution, "download", Pos(0), "downloads a remote file to disk"),
    r("socket.socket",           Net, Notable, "socket", NoArg),
    r("socket.create_connection", Net, Notable, "connect", Pos(0)),
    r("http.client.HTTPConnection", Net, Notable, "connect", Pos(0)),
    r("http.client.HTTPSConnection", Net, Notable, "connect", Pos(0)),
    r("ftplib.FTP",              Net, Notable, "connect", Pos(0)),
    r("smtplib.SMTP",            Net, Notable, "mail",   Pos(0)),
    rn("paramiko.SSHClient",     Net, Caution, "ssh",    NoArg, "opens an SSH session"),
    r("boto3.client",            Net, Notable, "aws",    Pos(0)),
    r("boto3.resource",          Net, Notable, "aws",    Pos(0)),
    r("websockets.connect",      Net, Notable, "ws",     Pos(0)),
    r("urllib3.PoolManager",     Net, Notable, "session", NoArg),
    r("<session>.get",           Net, Notable, "GET",    Pos(0)),
    rn("<session>.post",         Net, Caution, "POST",   Pos(0), "sends data to a remote host"),
    r("<session>.request",       Net, Notable, "request", Pos(1)),
    rn("<session>.put",          Net, Caution, "PUT",    Pos(0), "sends data to a remote host"),
    rn("<session>.patch",        Net, Caution, "PATCH",  Pos(0), "sends data to a remote host"),
    rn("<session>.delete",       Net, Caution, "DELETE", Pos(0), "deletes a remote resource"),
    r("<session>.head",          Net, Info,    "HEAD",   Pos(0)),
    r("<session>.stream",        Net, Notable, "request", Pos(1)),
    r("<socket>.connect",        Net, Notable, "connect", Pos(0)),
    r("<socket>.connect_ex",     Net, Notable, "connect", Pos(0)),
    rn("<socket>.bind",          Net, Notable, "listen", Pos(0), "opens a port"),
    rn("<socket>.listen",        Net, Notable, "listen", NoArg,  "opens a port"),
    r("<socket>.send",           Net, Notable, "send",   NoArg),
    r("<socket>.sendall",        Net, Notable, "send",   NoArg),
    r("<socket>.sendto",         Net, Notable, "send",   Pos(1)),
    r("<socket>.recv",           Net, Info,    "recv",   NoArg),
    r("httpx.put",               Net, Caution, "PUT",    Pos(0)),
    r("httpx.patch",             Net, Caution, "PATCH",  Pos(0)),
    r("httpx.delete",            Net, Caution, "DELETE", Pos(0)),
    r("httpx.request",           Net, Notable, "request", Pos(1)),
    r("httpx.stream",            Net, Notable, "request", Pos(1)),
    r("aiohttp.request",         Net, Notable, "request", Pos(1)),
    r("urllib.request.Request",  Net, Info,    "request", Pos(0)),
    r("webbrowser.open",         Net, Notable, "browse", Pos(0)),
    r("webbrowser.open_new",     Net, Notable, "browse", Pos(0)),
    r("xmlrpc.client.ServerProxy", Net, Notable, "rpc",  Pos(0)),
    r("imaplib.IMAP4",           Net, Notable, "mail",   Pos(0)),
    r("imaplib.IMAP4_SSL",       Net, Notable, "mail",   Pos(0)),
    r("poplib.POP3",             Net, Notable, "mail",   Pos(0)),
    r("smtplib.SMTP_SSL",        Net, Notable, "mail",   Pos(0)),
    rn("http.server.HTTPServer", Net, Notable, "listen", Pos(0), "opens a port"),
    rn("socketserver.TCPServer", Net, Notable, "listen", Pos(0), "opens a port"),
    r("psycopg2.connect",        Net, Notable, "connect", NoArg),
    r("pymysql.connect",         Net, Notable, "connect", NoArg),
    r("pymongo.MongoClient",     Net, Notable, "connect", Pos(0)),
    r("redis.Redis",             Net, Notable, "connect", NoArg),
    r("sqlalchemy.create_engine", Net, Notable, "connect", Pos(0)),
    r("openai.OpenAI",           Net, Notable, "api",    NoArg),
    r("anthropic.Anthropic",     Net, Notable, "api",    NoArg),
    r("huggingface_hub.hf_hub_download", Net, Notable, "download", Pos(0)),
    r("huggingface_hub.snapshot_download", Net, Notable, "download", Pos(0)),
    r("transformers.AutoModel.from_pretrained", Net, Notable, "download", Pos(0)),
    r("transformers.AutoTokenizer.from_pretrained", Net, Notable, "download", Pos(0)),
    r("transformers.AutoModelForCausalLM.from_pretrained", Net, Notable, "download", Pos(0)),
    r("torch.hub.load",          Net, Notable, "download", Pos(0)),
    r("ftplib.FTP_TLS",          Net, Notable, "connect", Pos(0)),
    rn("telnetlib.Telnet",       Net, Caution, "telnet", Pos(0), "opens an unencrypted session"),

    // ---- subprocess / shell ------------------------------------------------
    rn("subprocess.run",         Process, Caution, "run",   Pos(0), "runs an external command"),
    rn("subprocess.Popen",       Process, Caution, "spawn", Pos(0), "spawns an external process"),
    rn("subprocess.call",        Process, Caution, "run",   Pos(0), "runs an external command"),
    rn("subprocess.check_call",  Process, Caution, "run",   Pos(0), "runs an external command"),
    rn("subprocess.check_output", Process, Caution, "run",  Pos(0), "runs an external command"),
    rn("subprocess.getoutput",   Process, Caution, "shell", Pos(0), "runs a command through the shell"),
    rn("subprocess.getstatusoutput", Process, Caution, "shell", Pos(0), "runs a command through the shell"),
    rn("os.system",              Process, Caution, "shell", Pos(0), "runs a command through the shell"),
    rn("os.popen",               Process, Caution, "shell", Pos(0), "runs a command through the shell"),
    rn("os.execv",               Process, Caution, "exec",  Pos(0), "replaces this process"),
    rn("os.execvp",              Process, Caution, "exec",  Pos(0), "replaces this process"),
    rn("os.fork",                Process, Caution, "fork",  NoArg,  "forks the process"),
    rn("pty.spawn",              Process, Caution, "spawn", Pos(0), "spawns an interactive process"),
    rn("os.execl",               Process, Caution, "exec",  Pos(0), "replaces this process"),
    rn("os.execle",              Process, Caution, "exec",  Pos(0), "replaces this process"),
    rn("os.execlp",              Process, Caution, "exec",  Pos(0), "replaces this process"),
    rn("os.execlpe",             Process, Caution, "exec",  Pos(0), "replaces this process"),
    rn("os.execve",              Process, Caution, "exec",  Pos(0), "replaces this process"),
    rn("os.execvpe",             Process, Caution, "exec",  Pos(0), "replaces this process"),
    rn("os.spawnl",              Process, Caution, "spawn", Pos(1), "spawns an external process"),
    rn("os.spawnle",             Process, Caution, "spawn", Pos(1), "spawns an external process"),
    rn("os.spawnlp",             Process, Caution, "spawn", Pos(1), "spawns an external process"),
    rn("os.spawnv",              Process, Caution, "spawn", Pos(1), "spawns an external process"),
    rn("os.spawnve",             Process, Caution, "spawn", Pos(1), "spawns an external process"),
    rn("os.spawnvp",             Process, Caution, "spawn", Pos(1), "spawns an external process"),
    rn("os.posix_spawn",         Process, Caution, "spawn", Pos(0), "spawns an external process"),
    rn("os.posix_spawnp",        Process, Caution, "spawn", Pos(0), "spawns an external process"),
    rn("os.forkpty",             Process, Caution, "fork",  NoArg,  "forks the process"),
    rn("os.kill",                Process, Caution, "kill",  Pos(0), "signals another process"),
    rn("os.killpg",              Process, Caution, "kill",  Pos(0), "signals a process group"),
    rn("os.startfile",           Process, Caution, "open",  Pos(0), "opens a file with its default application"),
    rn("os.setuid",              Process, Caution, "setuid", Pos(0), "changes the process's user"),
    rn("os.setgid",              Process, Caution, "setgid", Pos(0), "changes the process's group"),
    rn("asyncio.create_subprocess_exec", Process, Caution, "spawn", Pos(0), "spawns an external process"),
    rn("asyncio.create_subprocess_shell", Process, Caution, "shell", Pos(0), "runs a command through the shell"),
    rn("multiprocessing.Process.start", Process, Notable, "start", NoArg, "starts a worker process"),
    r("<proc>.communicate",      Process, Info,    "await", NoArg),
    r("<proc>.wait",             Process, Info,    "await", NoArg),
    rn("<proc>.kill",            Process, Notable, "kill",  NoArg, "kills the child process"),
    rn("<proc>.terminate",       Process, Notable, "kill",  NoArg, "terminates the child process"),
    rn("<proc>.send_signal",     Process, Notable, "signal", Pos(0), "signals the child process"),
    r("signal.signal",           Process, Info,    "handler", Pos(0)),
    rn("signal.raise_signal",    Process, Notable, "signal", Pos(0), "raises a signal"),
    rn("shutil.which",           Process, Info,    "which", Pos(0), "locates an executable"),
    rn("pexpect.spawn",          Process, Caution, "spawn", Pos(0), "spawns an interactive process"),
    rn("fabric.Connection",      Net,     Caution, "ssh",   Pos(0), "opens an SSH session"),
    rn("docker.from_env",        Process, Caution, "docker", NoArg, "talks to the Docker daemon"),
    rn("sh.Command",             Process, Caution, "run",   Pos(0), "runs an external command"),
    rn("plumbum.local",          Process, Caution, "run",   Pos(0), "runs an external command"),

    // ---- environment / credentials ----------------------------------------
    r("os.getenv",               Env, Info,    "getenv", Pos(0)),
    r("os.putenv",               Env, Notable, "setenv", Pos(0)),
    r("os.environ.get",          Env, Info,    "getenv", Pos(0)),
    r("os.environ.setdefault",   Env, Notable, "setenv", Pos(0)),
    r("dotenv.load_dotenv",      Env, Notable, "dotenv", Pos(0)),
    rn("getpass.getpass",        Env, Notable, "prompt", Pos(0), "reads a secret from the terminal"),
    r("platform.system",         Env, Info,    "probe",  NoArg),
    r("os.uname",                Env, Info,    "probe",  NoArg),
    r("os.getcwd",               Env, Info,    "probe",  NoArg),
    r("os.chdir",                Env, Notable, "chdir",  Pos(0)),
    r("os.environ.update",       Env, Notable, "setenv", NoArg),
    rn("os.environ.pop",         Env, Notable, "unsetenv", Pos(0), "removes an environment variable"),
    rn("os.environ.clear",       Env, Caution, "unsetenv", NoArg, "clears the whole environment"),
    rn("os.unsetenv",            Env, Notable, "unsetenv", Pos(0), "removes an environment variable"),
    rn("sys.path.insert",        Env, Notable, "syspath", Pos(1), "changes where imports come from"),
    rn("sys.path.append",        Env, Notable, "syspath", Pos(0), "changes where imports come from"),
    rn("sys.path.extend",        Env, Notable, "syspath", Pos(0), "changes where imports come from"),
    r("dotenv.dotenv_values",    Env, Notable, "dotenv", Pos(0)),
    r("input",                   Env, Info,    "prompt", Pos(0)),
    r("os.getlogin",             Env, Info,    "probe",  NoArg),
    r("os.getpid",               Env, Info,    "probe",  NoArg),
    r("socket.gethostname",      Env, Info,    "probe",  NoArg),
    r("platform.platform",       Env, Info,    "probe",  NoArg),
    r("platform.node",           Env, Info,    "probe",  NoArg),
    r("sys.getrecursionlimit",   Env, Info,    "probe",  NoArg),
    rn("sys.setrecursionlimit",  Env, Notable, "limit",  Pos(0), "raises the recursion limit"),
    rn("resource.setrlimit",     Env, Notable, "limit",  Pos(0), "changes a resource limit"),
    rn("os.umask",               Env, Notable, "umask",  Pos(0), "changes default file permissions"),
    rn("keyring.get_password",   Env, Caution, "keyring", Pos(0), "reads a stored password"),
    rn("netrc.netrc",            Env, Caution, "netrc",  NoArg,  "reads stored credentials"),
    rn("boto3.Session",          Env, Notable, "aws",    NoArg,  "loads cloud credentials"),

    // ---- dynamic code ------------------------------------------------------
    rn("eval",                   Dynamic, Caution, "eval",   Pos(0), "evaluates code built at runtime"),
    rn("exec",                   Dynamic, Caution, "exec",   Pos(0), "executes code built at runtime"),
    rn("compile",                Dynamic, Caution, "compile", Pos(0), "compiles code built at runtime"),
    rn("__import__",             Dynamic, Caution, "import", Pos(0), "imports a module chosen at runtime"),
    r("importlib.import_module", Dynamic, Notable, "import", Pos(0)),
    rn("pickle.loads",           Dynamic, Caution, "unpickle", NoArg, "unpickling runs arbitrary code"),
    rn("pickle.load",            Dynamic, Caution, "unpickle", Pos(0), "unpickling runs arbitrary code"),
    rn("dill.loads",             Dynamic, Caution, "unpickle", NoArg, "unpickling runs arbitrary code"),
    rn("marshal.loads",          Dynamic, Caution, "unmarshal", NoArg, "decodes raw code objects"),
    rn("yaml.load",              Dynamic, Caution, "yaml",   Pos(0), "yaml.load without SafeLoader can construct objects"),
    rn("torch.load",             Dynamic, Caution, "load",   Pos(0), "torch.load unpickles by default"),
    rn("ctypes.CDLL",            Dynamic, Caution, "dlopen", Pos(0), "loads native code"),
    rn("ctypes.cdll.LoadLibrary", Dynamic, Caution, "dlopen", Pos(0), "loads native code"),
    rn("ctypes.WinDLL",          Dynamic, Caution, "dlopen", Pos(0), "loads native code"),
    rn("ctypes.PyDLL",           Dynamic, Caution, "dlopen", Pos(0), "loads native code"),
    rn("ctypes.OleDLL",          Dynamic, Caution, "dlopen", Pos(0), "loads native code"),
    rn("ctypes.windll.LoadLibrary", Dynamic, Caution, "dlopen", Pos(0), "loads native code"),
    r("ctypes.util.find_library", Dynamic, Info,   "dlfind", Pos(0)),
    rn("marshal.load",           Dynamic, Caution, "unmarshal", Pos(0), "decodes raw code objects"),
    rn("dill.load",              Dynamic, Caution, "unpickle", Pos(0), "unpickling runs arbitrary code"),
    rn("joblib.load",            Dynamic, Caution, "unpickle", Pos(0), "joblib.load unpickles"),
    rn("cloudpickle.load",       Dynamic, Caution, "unpickle", Pos(0), "unpickling runs arbitrary code"),
    rn("cloudpickle.loads",      Dynamic, Caution, "unpickle", NoArg,  "unpickling runs arbitrary code"),
    rn("pickle.Unpickler",       Dynamic, Caution, "unpickle", Pos(0), "unpickling runs arbitrary code"),
    rn("pandas.read_pickle",     Dynamic, Caution, "unpickle", Pos(0), "unpickling runs arbitrary code"),
    rn("numpy.lib.format.read_array", Dynamic, Notable, "load", Pos(0), "may unpickle"),
    rn("yaml.unsafe_load",       Dynamic, Caution, "yaml",   Pos(0), "yaml.unsafe_load can construct arbitrary objects"),
    rn("yaml.full_load",         Dynamic, Caution, "yaml",   Pos(0), "yaml.full_load can construct objects"),
    rn("yaml.load_all",          Dynamic, Caution, "yaml",   Pos(0), "yaml.load_all without SafeLoader can construct objects"),
    rn("importlib.reload",       Dynamic, Notable, "reload", Pos(0), "re-executes a module"),
    rn("importlib.util.spec_from_file_location", Dynamic, Notable, "import", Pos(1), "imports a file as a module"),
    rn("runpy.run_path",         Dynamic, Caution, "run",    Pos(0), "executes a file as a script"),
    rn("runpy.run_module",       Dynamic, Caution, "run",    Pos(0), "executes a module as a script"),
    rn("code.interact",          Dynamic, Notable, "repl",   NoArg,  "opens an interactive console"),
    rn("code.InteractiveConsole", Dynamic, Notable, "repl",  NoArg,  "opens an interactive console"),
    rn("pdb.set_trace",          Dynamic, Notable, "debug",  NoArg,  "stops in the debugger"),
    rn("breakpoint",             Dynamic, Notable, "debug",  NoArg,  "stops in the debugger"),
    rn("types.FunctionType",     Dynamic, Caution, "codeobj", Pos(0), "builds a function from a code object"),
    rn("builtins.eval",          Dynamic, Caution, "eval",   Pos(0), "evaluates code built at runtime"),
    rn("builtins.exec",          Dynamic, Caution, "exec",   Pos(0), "executes code built at runtime"),
    rn("builtins.__import__",    Dynamic, Caution, "import", Pos(0), "imports a module chosen at runtime"),
    rn("setattr",                Dynamic, Info,    "setattr", Pos(1), "sets an attribute chosen at runtime"),
    rn("jsonpickle.decode",      Dynamic, Caution, "unpickle", Pos(0), "jsonpickle can construct arbitrary objects"),
    rn("xml.etree.ElementTree.parse", FsRead, Info, "parse", Pos(0), "XML parsing"),
    rn("atexit.register",        Dynamic, Info,    "atexit", Pos(0), "runs code at interpreter exit"),

    // ---- output -----------------------------------------------------------
    r("print",                   Stdout, Info, "print", Pos(0)),
    r("pprint.pprint",           Stdout, Info, "print", Pos(0)),
    r("rich.print",              Stdout, Info, "print", Pos(0)),
    r("sys.stdout.write",        Stdout, Info, "print", Pos(0)),
    r("sys.stderr.write",        Stdout, Info, "warn",  Pos(0)),
    r("logging.debug",           Stdout, Info, "log",   Pos(0)),
    r("logging.info",            Stdout, Info, "log",   Pos(0)),
    r("logging.warning",         Stdout, Info, "log",   Pos(0)),
    r("logging.error",           Stdout, Info, "log",   Pos(0)),
    r("logging.exception",       Stdout, Info, "log",   Pos(0)),
    r("tqdm.tqdm",               Stdout, Info, "progress", NoArg),
    r("matplotlib.pyplot.show",  Stdout, Info, "plot",  NoArg),

    // ---- randomness --------------------------------------------------------
    r("random.random",           Random, Info, "random", NoArg),
    r("random.randint",          Random, Info, "random", NoArg),
    r("random.choice",           Random, Info, "random", NoArg),
    r("random.shuffle",          Random, Info, "shuffle", NoArg),
    r("random.sample",           Random, Info, "sample", NoArg),
    r("random.seed",             Random, Info, "seed",   Pos(0)),
    r("numpy.random.rand",       Random, Info, "random", NoArg),
    r("numpy.random.randn",      Random, Info, "random", NoArg),
    r("numpy.random.randint",    Random, Info, "random", NoArg),
    r("numpy.random.seed",       Random, Info, "seed",   Pos(0)),
    r("numpy.random.default_rng", Random, Info, "rng",   Pos(0)),
    r("secrets.token_hex",       Random, Info, "secret", NoArg),
    r("secrets.token_urlsafe",   Random, Info, "secret", NoArg),
    r("uuid.uuid4",              Random, Info, "uuid",   NoArg),
    r("os.urandom",              Random, Info, "random", NoArg),
    r("torch.manual_seed",       Random, Info, "seed",   Pos(0)),

    // ---- time --------------------------------------------------------------
    rn("time.sleep",             Clock, Notable, "sleep", Pos(0), "blocks for a while"),
    r("time.time",               Clock, Info, "now",    NoArg),
    r("time.perf_counter",       Clock, Info, "clock",  NoArg),
    r("time.monotonic",          Clock, Info, "clock",  NoArg),
    r("datetime.datetime.now",   Clock, Info, "now",    NoArg),
    r("datetime.date.today",     Clock, Info, "today",  NoArg),
    r("asyncio.sleep",           Clock, Info, "sleep",  Pos(0)),

    // ---- concurrency -------------------------------------------------------
    r("threading.Thread",        Concurrency, Notable, "thread", NoArg),
    r("multiprocessing.Pool",    Concurrency, Notable, "pool",   Pos(0)),
    r("multiprocessing.Process", Concurrency, Notable, "process", NoArg),
    r("concurrent.futures.ThreadPoolExecutor", Concurrency, Notable, "pool", NoArg),
    r("concurrent.futures.ProcessPoolExecutor", Concurrency, Notable, "pool", NoArg),
    r("asyncio.run",             Concurrency, Info, "async",  NoArg),
    r("asyncio.gather",          Concurrency, Info, "gather", NoArg),
    r("asyncio.create_task",     Concurrency, Info, "task",   NoArg),
    r("joblib.Parallel",         Concurrency, Notable, "parallel", NoArg),

    // ---- termination -------------------------------------------------------
    rn("sys.exit",               Exit, Notable, "exit", Pos(0), "ends the process"),
    rn("os._exit",               Exit, Caution, "exit", Pos(0), "ends the process immediately, skipping cleanup"),
    rn("exit",                   Exit, Notable, "exit", Pos(0), "ends the process"),
    rn("quit",                   Exit, Notable, "exit", Pos(0), "ends the process"),
];

/// Methods distinctive enough to name their effect even when we could not
/// work out what they were called on. `stats.to_csv(path)` writes a file
/// whether or not we managed to prove `stats` is a DataFrame; missing that is
/// worse than the occasional false positive on a same-named method.
#[rustfmt::skip]
pub const METHOD_RULES: &[Rule] = &[
    r("to_csv",      FsWrite, Notable, "write",  Pos(0)),
    r("to_parquet",  FsWrite, Notable, "write",  Pos(0)),
    r("to_json",     FsWrite, Notable, "write",  Pos(0)),
    r("to_excel",    FsWrite, Notable, "write",  Pos(0)),
    r("to_pickle",   FsWrite, Notable, "write",  Pos(0)),
    r("to_sql",      FsWrite, Notable, "write",  Pos(0)),
    r("to_hdf",      FsWrite, Notable, "write",  Pos(0)),
    r("savefig",     FsWrite, Notable, "plot",   Pos(0)),
    r("write_text",  FsWrite, Notable, "write",  NoArg),
    r("write_bytes", FsWrite, Notable, "write",  NoArg),
    r("read_text",   FsRead,  Info,    "read",   NoArg),
    r("read_bytes",  FsRead,  Info,    "read",   NoArg),
    r("iterdir",     FsRead,  Info,    "list",   NoArg),
    r("rglob",       FsRead,  Info,    "glob",   Pos(0)),
    rn("unlink",     FsDelete, Caution, "delete", NoArg, "removes a file from disk"),
    rn("rmdir",      FsDelete, Caution, "rmdir",  NoArg, "removes a directory"),
];

fn method_index() -> &'static HashMap<&'static str, &'static Rule> {
    static IDX: OnceLock<HashMap<&'static str, &'static Rule>> = OnceLock::new();
    IDX.get_or_init(|| METHOD_RULES.iter().map(|rule| (rule.path, rule)).collect())
}

/// Look up a bare method name. Only consulted once the fully-qualified lookup
/// has failed, so a resolved receiver always wins.
pub fn method_lookup(method: &str) -> Option<&'static Rule> {
    method_index().get(method).copied()
}

fn index() -> &'static HashMap<&'static str, &'static Rule> {
    static IDX: OnceLock<HashMap<&'static str, &'static Rule>> = OnceLock::new();
    IDX.get_or_init(|| RULES.iter().map(|rule| (rule.path, rule)).collect())
}

pub fn lookup(path: &str) -> Option<&'static Rule> {
    index().get(path).copied()
}

/// Modules whose calls count as "compute" when no more specific rule matched.
/// These are the libraries whose presence tells you the snippet is doing
/// numerical work, which is worth showing even without a per-symbol rule.
pub const COMPUTE_MODULES: &[&str] = &[
    "numpy",
    "pandas",
    "scipy",
    "torch",
    "tensorflow",
    "jax",
    "sklearn",
    "statsmodels",
    "polars",
    "xarray",
    "sympy",
    "numba",
    "cupy",
    "dask",
    "matplotlib",
    "seaborn",
    "plotly",
    "networkx",
    "MDAnalysis",
    "mdtraj",
    "Bio",
    "openmm",
    "ase",
    "rdkit",
];

/// Modules that ship with CPython. Anything else needs installing, which is
/// itself worth surfacing when an LLM hands you a snippet to run.
pub const STDLIB: &[&str] = &[
    "abc",
    "argparse",
    "array",
    "ast",
    "asyncio",
    "base64",
    "binascii",
    "bisect",
    "builtins",
    "bz2",
    "calendar",
    "cmath",
    "cmd",
    "collections",
    "colorsys",
    "concurrent",
    "configparser",
    "contextlib",
    "copy",
    "csv",
    "ctypes",
    "dataclasses",
    "datetime",
    "decimal",
    "difflib",
    "dis",
    "email",
    "enum",
    "errno",
    "faulthandler",
    "filecmp",
    "fileinput",
    "fnmatch",
    "fractions",
    "ftplib",
    "functools",
    "gc",
    "getpass",
    "gettext",
    "glob",
    "graphlib",
    "gzip",
    "hashlib",
    "heapq",
    "hmac",
    "html",
    "http",
    "imaplib",
    "importlib",
    "inspect",
    "io",
    "ipaddress",
    "itertools",
    "json",
    "keyword",
    "linecache",
    "locale",
    "logging",
    "lzma",
    "marshal",
    "math",
    "mimetypes",
    "mmap",
    "multiprocessing",
    "netrc",
    "numbers",
    "operator",
    "os",
    "pathlib",
    "pickle",
    "pickletools",
    "pkgutil",
    "platform",
    "plistlib",
    "poplib",
    "posixpath",
    "pprint",
    "profile",
    "pty",
    "queue",
    "quopri",
    "random",
    "re",
    "readline",
    "reprlib",
    "resource",
    "runpy",
    "sched",
    "secrets",
    "select",
    "selectors",
    "shelve",
    "shlex",
    "shutil",
    "signal",
    "site",
    "smtplib",
    "socket",
    "socketserver",
    "sqlite3",
    "ssl",
    "stat",
    "statistics",
    "string",
    "stringprep",
    "struct",
    "subprocess",
    "sys",
    "sysconfig",
    "tarfile",
    "tempfile",
    "termios",
    "textwrap",
    "threading",
    "time",
    "timeit",
    "tkinter",
    "token",
    "tokenize",
    "tomllib",
    "trace",
    "traceback",
    "tracemalloc",
    "tty",
    "types",
    "typing",
    "unicodedata",
    "unittest",
    "urllib",
    "uuid",
    "venv",
    "warnings",
    "wave",
    "weakref",
    "webbrowser",
    "wsgiref",
    "xml",
    "xmlrpc",
    "zipapp",
    "zipfile",
    "zipimport",
    "zlib",
    "zoneinfo",
    "__future__",
];

pub fn is_stdlib(module: &str) -> bool {
    let root = module.split('.').next().unwrap_or(module);
    STDLIB.contains(&root)
}

pub fn is_compute_module(module: &str) -> bool {
    let root = module.split('.').next().unwrap_or(module);
    COMPUTE_MODULES.contains(&root)
}

/// The value kind a call of `path` produces, so later method calls on the
/// result can be resolved. `open(p)` yields a `<file>`, `Path(p)` a `<path>`.
pub fn produces(path: &str) -> ValueKind {
    match path {
        "open" | "io.open" | "gzip.open" | "bz2.open" | "lzma.open" | "codecs.open" => {
            ValueKind::File
        }
        "tempfile.NamedTemporaryFile" | "tempfile.TemporaryFile" => ValueKind::File,
        "pathlib.Path" | "pathlib.PurePath" | "pathlib.PosixPath" => ValueKind::Path,
        "requests.Session"
        | "httpx.Client"
        | "httpx.AsyncClient"
        | "aiohttp.ClientSession"
        | "urllib3.PoolManager" => ValueKind::Session,
        "requests.get"
        | "requests.post"
        | "requests.put"
        | "requests.patch"
        | "requests.delete"
        | "requests.head"
        | "requests.request"
        | "httpx.get"
        | "httpx.post"
        | "httpx.put"
        | "httpx.patch"
        | "httpx.delete"
        | "httpx.request"
        | "<session>.get"
        | "<session>.post"
        | "<session>.put"
        | "<session>.patch"
        | "<session>.delete"
        | "<session>.request"
        | "urllib.request.urlopen" => ValueKind::Response,
        "subprocess.run"
        | "subprocess.Popen"
        | "asyncio.create_subprocess_exec"
        | "asyncio.create_subprocess_shell"
        | "pexpect.spawn" => ValueKind::Process,
        "socket.socket" | "socket.create_connection" => ValueKind::Socket,
        "sqlite3.connect"
        | "psycopg2.connect"
        | "pymysql.connect"
        | "<conn>.cursor"
        | "sqlalchemy.create_engine"
        | "duckdb.connect" => ValueKind::Connection,
        "zipfile.ZipFile" | "tarfile.open" | "tarfile.TarFile" => ValueKind::Archive,
        "<path>.open" => ValueKind::File,
        "pandas.DataFrame"
        | "pandas.read_csv"
        | "pandas.read_parquet"
        | "pandas.read_json"
        | "pandas.read_excel"
        | "pandas.read_hdf"
        | "pandas.read_table"
        | "pandas.read_sql"
        | "polars.read_csv"
        | "polars.DataFrame" => ValueKind::Frame,
        "numpy.array" | "numpy.zeros" | "numpy.ones" | "numpy.arange" | "numpy.linspace"
        | "numpy.empty" | "numpy.load" | "numpy.loadtxt" | "numpy.genfromtxt" | "numpy.asarray"
        | "numpy.full" | "numpy.eye" => ValueKind::Array,
        "torch.tensor" | "torch.zeros" | "torch.ones" | "torch.randn" | "torch.from_numpy"
        | "torch.load" => ValueKind::Tensor,
        "str" | "repr" | "format" | "input" => ValueKind::Text,
        "int" | "float" | "len" | "sum" | "abs" | "round" | "min" | "max" | "ord" => {
            ValueKind::Number
        }
        "bool" | "isinstance" | "hasattr" | "callable" | "any" | "all" => ValueKind::Bool,
        "list" | "sorted" | "tuple" | "set" | "range" | "enumerate" | "zip" | "map" | "filter"
        | "reversed" | "glob.glob" | "os.listdir" => ValueKind::Seq,
        "dict"
        | "json.load"
        | "json.loads"
        | "collections.Counter"
        | "collections.defaultdict"
        | "collections.OrderedDict" => ValueKind::Map,
        _ => ValueKind::Unknown,
    }
}

/// Namespace prefix used to key method rules for an inferred value kind.
pub fn kind_prefix(kind: ValueKind) -> Option<&'static str> {
    match kind {
        ValueKind::File => Some("<file>"),
        ValueKind::Path => Some("<path>"),
        ValueKind::Session => Some("<session>"),
        ValueKind::Response => Some("<response>"),
        ValueKind::Frame => Some("<frame>"),
        ValueKind::Process => Some("<proc>"),
        ValueKind::Socket => Some("<socket>"),
        ValueKind::Connection => Some("<conn>"),
        ValueKind::Archive => Some("<archive>"),
        _ => None,
    }
}

/// Shell fragments and paths that turn an ordinary command into one worth
/// reading twice. Matched case-insensitively against string literals that
/// reach a process- or filesystem-flavoured call.
pub const HAZARD_STRINGS: &[(&str, &str)] = &[
    ("rm -rf", "recursive delete"),
    ("rm -fr", "recursive delete"),
    ("sudo ", "elevates privileges"),
    ("chmod 777", "makes a path world-writable"),
    ("mkfs", "formats a filesystem"),
    ("dd if=", "raw device write"),
    ("| sh", "pipes downloaded content into a shell"),
    ("| bash", "pipes downloaded content into a shell"),
    ("curl ", "downloads from the network"),
    ("wget ", "downloads from the network"),
    ("/etc/passwd", "reads a system credential file"),
    ("/etc/shadow", "reads a system credential file"),
    (".ssh/", "touches SSH keys"),
    (".aws/credentials", "touches cloud credentials"),
    ("~/.netrc", "touches stored credentials"),
    ("pip install", "installs packages"),
    ("apt-get", "changes system packages"),
    ("git push", "publishes to a remote"),
    ("--force", "forces an otherwise-refused operation"),
    (".pyxray.toml", "changes pyxray's own policy"),
    ("pyxray/policy.toml", "changes pyxray's own policy"),
];

/// What a keyword argument's value looked like at the call site, in the only
/// terms a hazard rule cares about.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum KwValue {
    /// A literal `True` (or a non-zero integer).
    True,
    /// A literal `False`, `None` or `0`.
    False,
    /// A name or attribute we could resolve, e.g. `yaml.Loader`.
    Path(String),
    /// Anything else: a call, an expression, a variable we know nothing about.
    Other,
}

impl KwValue {
    /// True, or something decided at runtime — either way, not the safe value.
    fn maybe_true(&self) -> bool {
        !matches!(self, KwValue::False)
    }
    fn is_literal(&self) -> bool {
        matches!(self, KwValue::True | KwValue::False)
    }
}

/// What a keyword-argument hazard changes about the reading of a call.
pub struct KwHazard {
    pub note: &'static str,
    pub severity: Severity,
    /// Some hazards change *what* the call does, not just how carefully to
    /// read it: `numpy.load(allow_pickle=True)` is dynamic code, not a read.
    pub effect: Option<Effect>,
}

const fn kh(note: &'static str, severity: Severity) -> KwHazard {
    KwHazard {
        note,
        severity,
        effect: None,
    }
}

/// Argument names whose presence changes how a call should be read. A value
/// that is not a literal still counts, at one notch lower: `shell=use_shell`
/// is a shell call whenever someone sets the flag, and the reader should know
/// the flag exists.
pub fn kwarg_hazard(callee: &str, kw: &str, value: &KwValue) -> Option<KwHazard> {
    let literal = value.is_literal();
    let on = value.maybe_true();
    let off = matches!(value, KwValue::False) || (!literal && kw != "shell");
    let is_http = callee.starts_with("requests.")
        || callee.starts_with("httpx.")
        || callee.starts_with("<session>.")
        || callee.starts_with("aiohttp.")
        || callee.starts_with("urllib3.");
    let is_proc =
        callee.starts_with("subprocess.") || callee.starts_with("asyncio.create_subprocess");
    match kw {
        "shell" if is_proc && on => Some(if literal {
            kh(
                "shell=True — the argument is interpreted by /bin/sh",
                Severity::Caution,
            )
        } else {
            kh(
                "shell=… — decided at runtime; may go through /bin/sh",
                Severity::Notable,
            )
        }),
        "check" if is_proc && matches!(value, KwValue::False) => Some(kh(
            "check=False — a failing command is ignored",
            Severity::Notable,
        )),
        "verify" if is_http && off => Some(if literal {
            kh(
                "verify=False — TLS certificates are not checked",
                Severity::Caution,
            )
        } else {
            kh(
                "verify=… — decided at runtime; TLS checking may be off",
                Severity::Notable,
            )
        }),
        "allow_pickle" if callee == "numpy.load" && on => Some(KwHazard {
            note: "allow_pickle=True — loading can run arbitrary code",
            severity: Severity::Caution,
            effect: Some(Effect::Dynamic),
        }),
        "weights_only" if callee == "torch.load" && matches!(value, KwValue::False) => Some(kh(
            "weights_only=False — torch.load will unpickle arbitrary objects",
            Severity::Caution,
        )),
        "trust_remote_code" if on => Some(KwHazard {
            note: "trust_remote_code=True — runs code shipped with the download",
            severity: Severity::Caution,
            effect: Some(Effect::Dynamic),
        }),
        "Loader" if callee.starts_with("yaml.") => match value {
            KwValue::Path(p) if p.ends_with("SafeLoader") || p.ends_with("CSafeLoader") => None,
            KwValue::Path(p) if p.ends_with("Loader") => Some(KwHazard {
                note: "Loader=yaml.Loader — can construct arbitrary Python objects",
                severity: Severity::Caution,
                effect: Some(Effect::Dynamic),
            }),
            _ => None,
        },
        "ignore_errors" if on && literal => Some(kh(
            "ignore_errors=True — failures pass silently",
            Severity::Notable,
        )),
        "preexec_fn" if is_proc => Some(kh(
            "preexec_fn — runs a callable in the child before exec",
            Severity::Notable,
        )),
        "onerror" | "onexc" if callee == "shutil.rmtree" => None,
        "dir_fd" => None,
        _ => None,
    }
}

/// Does an environment-variable name look like it holds a credential?
/// Case-insensitive substring match; deliberately generous, because the cost
/// of a false alarm here is one amber row and the cost of a miss is a leaked
/// key.
/// Path fragments that name a credential file. Reading one is how a secret
/// enters a snippet as surely as an environment variable, and the file kind
/// matters more: a token belongs to the API it is sent to, a private key
/// belongs nowhere else.
pub const SECRET_PATHS: &[&str] = &[
    ".ssh/",
    "id_rsa",
    "id_ed25519",
    "id_ecdsa",
    "id_dsa",
    ".aws/credentials",
    ".aws/config",
    ".netrc",
    ".git-credentials",
    ".kube/config",
    ".docker/config.json",
    ".pypirc",
    ".npmrc",
    ".config/gh/hosts.yml",
    ".gnupg/",
    "/etc/shadow",
];

/// Whether a path names a credential file. `.env` counts only as the whole
/// file name (`.env`, `prod.env`, `.env.local`), so `.envrc` and `venv` do not.
pub fn is_secret_path(path: &str) -> bool {
    let p = path.replace('\\', "/").to_ascii_lowercase();
    if SECRET_PATHS.iter().any(|needle| p.contains(needle)) {
        return true;
    }
    let name = p.rsplit('/').next().unwrap_or(&p);
    name == ".env" || name.starts_with(".env.") || name.ends_with(".env")
}

pub fn is_secret_name(name: &str) -> bool {
    const HINTS: &[&str] = &[
        "token",
        "secret",
        "passw",
        "api_key",
        "apikey",
        "api-key",
        "private",
        "credential",
        "auth",
        "cookie",
        "session_key",
        "access_key",
        "client_secret",
        "signing",
        "_pat",
        "bearer",
    ];
    let lower = name.to_ascii_lowercase();
    HINTS.iter().any(|h| lower.contains(h)) || lower.ends_with("_key") || lower == "key"
}
