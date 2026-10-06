#!/usr/bin/env python3
"""Rich Presence pessoal do daeese, usando a aplicacao do Sollarety.

Por que isto nao vive dentro do CommunityBot em C#: presenca de *usuario* nao
passa pelo gateway. Quem a define e o cliente Discord desktop, atraves de um
socket IPC local. Um bot de gateway, por definicao, nao tem como escrever na
presenca de uma conta - so a maquina onde o Discord esta aberto tem. Por isso
e um processo separado, embora reaproveite o mesmo Application ID.

Contexto: a intencao original era um widget custom de perfil. O Discord
encerrou aquele experimento (artigo oficial "Game Stats Widget Experiment",
27/07/2026). Medido nesta aplicacao em 21/08/2026, o servidor responde:

    POST /applications/1403153848507301939/widget-configs
    403  code 40128  "This action requires a claimed game on the application team"

O Sollarety e um bot de moderacao, nao um jogo reivindicado, entao aquele
caminho esta fechado. O cartao de Rich Presence e a superficie de perfil
programavel que restou - e essa nao depende de experimento nenhum.

Estados (decididos com o dono, textos e imagens em presence.json -> "states"):

    IDLE    nada detectado: o cartao fixo de sempre ("Watching Sollarety").
    CODING  um editor/agente de codigo esta com um repositorio aberto E algo
            mudou nele nos ultimos minutos. Mostra a linguagem e, so se o
            repositorio for PUBLICO no GitHub, o nome dele.
    VM      uma maquina virtual rodando. Tem prioridade e combina com CODING.
            Mostra so o SISTEMA da VM, nunca o nome dela (escolhido pelo dono).

A deteccao roda a cada ~15 s dentro da mesma sessao IPC; SET_ACTIVITY so sai
quando a atividade calculada muda (o Discord limita a 5 por 20 s).

Sem dependencias externas de proposito: o protocolo IPC e simples e a maquina
nao precisa de um venv so para isto.

Teste sem Discord:  python3 daeese_presence.py --dry-run
"""

import json
import os
import re
import signal
import socket
import struct
import subprocess
import sys
import time
import urllib.error
import urllib.request
import uuid
import xml.etree.ElementTree as ElementTree

OP_HANDSHAKE = 0
OP_FRAME = 1
OP_CLOSE = 2
OP_PING = 3
OP_PONG = 4

HERE = os.path.dirname(os.path.abspath(__file__))
CONFIG_PATH = os.path.join(HERE, "presence.json")

# Quanto tempo esperar antes de reconectar. Sobe ate o teto para nao martelar
# o socket enquanto o Discord esta fechado - o caso normal, nao um erro.
RECONNECT_MIN = 15
RECONNECT_MAX = 120

# Teto de espera numa leitura do socket IPC. O protocolo e conversa curta com o
# cliente local; passar disto significa que o outro lado travou. Vale para o
# handshake e para o MEIO de um frame; a espera ociosa entre frames usa o
# intervalo de varredura (ver read_exactly).
SOCKET_TIMEOUT = 30

# Maior frame aceito do cliente Discord. O campo de tamanho e um <I (ate 4 GiB)
# e vai direto para o read_exactly; os frames reais sao de alguns KB.
MAX_FRAME_BYTES = 1 << 20

# Intervalo padrao entre varreduras (presence.json "pollSeconds" sobrescreve).
POLL_SECONDS = 15
POLL_MIN_SECONDS = 5

# O Discord aceita 5 SET_ACTIVITY por 20 s. Com uma varredura a cada 15 s nunca
# chegamos perto, mas o piso protege contra um laco que mude a cada tick.
MIN_SEND_INTERVAL = 5

# Teto de qualquer subprocesso (virsh, git, osinfo-query...). Estourar vira
# "nada detectado", nunca excecao.
CMD_TIMEOUT = 5

# Janela de "mexeu recentemente": editor so aberto nao conta como programar.
RECENT_MINUTES = 20

# Caches. A API publica do GitHub sem token da 60 chamadas/h por IP.
LANG_CACHE_TTL = 3600
OS_CACHE_TTL = 3600
REMOTE_CACHE_TTL = 600
GITHUB_TTL = 6 * 3600
GITHUB_ERROR_TTL = 15 * 60

# Limites de seguranca da varredura de repositorios.
STATUS_MAX_ENTRIES = 5000
WORKSPACE_MAX_ENTRIES = 256

# Limites do Discord para os campos de texto da atividade.
TEXT_MAX = 128
BUTTON_LABEL_MAX = 32

IMAGE_BASE = "https://daeese.me/filearchive/presence/"

# Ids de badge que existem no site (contrato com filearchive/presence/). Um id
# fora daqui viraria imagem quebrada no cartao, entao cai para "generic".
LANG_BADGES = {
    "csharp", "cpp", "c", "rust", "python", "javascript", "typescript", "luau",
    "lua", "html", "css", "shell", "qml", "go", "java", "kotlin", "markdown", "generic",
}
OS_BADGES = {
    "windows11", "windows10", "windows", "gentoo", "fedora", "ubuntu", "debian",
    "arch", "linux", "macos", "freebsd", "generic",
}

# id -> nome mostrado no Discord.
LANGUAGE_NAMES = {
    "csharp": "C#",
    "cpp": "C++",
    "c": "C",
    "rust": "Rust",
    "python": "Python",
    "javascript": "JavaScript",
    "typescript": "TypeScript",
    "luau": "Luau",
    "lua": "Lua",
    "html": "HTML",
    "css": "CSS",
    "shell": "Shell",
    "qml": "QML",
    "go": "Go",
    "java": "Java",
    "kotlin": "Kotlin",
    "markdown": "Markdown",
    "generic": "code",
}

EXTENSION_LANGUAGES = {
    ".cs": "csharp",
    ".cpp": "cpp", ".cc": "cpp", ".cxx": "cpp", ".hpp": "cpp", ".hxx": "cpp", ".hh": "cpp",
    ".c": "c", ".h": "c",
    ".rs": "rust",
    ".py": "python", ".pyw": "python",
    ".js": "javascript", ".mjs": "javascript", ".cjs": "javascript", ".jsx": "javascript",
    ".ts": "typescript", ".tsx": "typescript", ".mts": "typescript", ".cts": "typescript",
    ".luau": "luau",
    ".lua": "lua",
    ".html": "html", ".htm": "html",
    ".css": "css",
    ".sh": "shell", ".bash": "shell", ".fish": "shell", ".zsh": "shell",
    ".qml": "qml",
    ".go": "go",
    ".java": "java",
    ".kt": "kotlin", ".kts": "kotlin",
    ".md": "markdown",
}

# Editores e agentes de codigo. Comparados com o comm do processo e com o
# basename do argv[0] (o comm do kernel corta em 15 caracteres).
EDITOR_NAMES = {
    "code", "code-oss", "codium", "vscodium", "code-insiders", "cursor", "windsurf",
    "zed", "zeditor", "zed-editor",
    "nvim", "vim", "vi", "gvim", "hx", "helix",
    "emacs", "emacs-gtk", "emacs-nox", "emacs-lucid",
    "kate", "sublime_text",
    "idea", "pycharm", "clion", "rider", "webstorm", "goland", "rustrover",
    "phpstorm", "rubymine", "datagrip", "fleet",
    # Claude Code CLI. Atencao: "claude-desktop" (o app) NAO casa, so o CLI.
    "claude",
}

# Servidores de linguagem. Varios rodam como node/python, entao tambem olhamos
# os primeiros argumentos (sem extensao).
LSP_NAMES = {
    "rust-analyzer", "clangd", "omnisharp", "csharp-ls", "roslyn",
    "microsoft.codeanalysis.languageserver", "pyright", "pyright-langserver",
    "basedpyright", "basedpyright-langserver", "pylsp", "typescript-language-server",
    "tsserver", "gopls", "lua-language-server", "luau-lsp", "qmlls", "qmlls6",
}

# Processos de VM fora do libvirt.
VBOX_NAMES = {"virtualboxvm", "vboxheadless"}
VMWARE_NAMES = {"vmware-vmx"}

UNKNOWN_OS = {"name": None, "badge": "generic"}

# Dominios do libosinfo que nao sao Linux (badge "generic" em vez de "linux").
NON_LINUX_DOMAINS = {
    "netbsd.org", "openbsd.org", "dragonflybsd.org", "oracle.com", "sun.com",
    "haiku-os.org", "reactos.org", "freedos.org", "novell.com", "ibm.com",
}

DEFAULT_STATES = {
    "coding": {
        "enabled": True,
        "recentMinutes": RECENT_MINUTES,
        "details": "Charting new stars",
        "state": "Writing {language} in {repo}",
        "statePrivate": "Writing {language} in a private project",
        "largeImage": IMAGE_BASE + "large/code.png",
        "largeText": "Sollarety",
        "smallImage": IMAGE_BASE + "lang/{badge}.png",
        "smallText": "{language}",
        # Tentados em ordem ate um caber em 32 caracteres.
        "repoButtonLabels": ["{repo} on GitHub", "GitHub: {repo}", "View on GitHub"],
    },
    "vm": {
        "enabled": True,
        "details": "Orbiting {os}",
        "detailsUnknown": "Orbiting a virtual machine",
        "state": "in a virtual machine",
        "stateMany": "in {count} virtual machines",
        "stateUnknown": "on an uncharted world",
        "stateCoding": "while writing {language} in {repo}",
        "stateCodingPrivate": "while writing {language} in a private project",
        "largeImage": IMAGE_BASE + "large/orbit.png",
        "largeText": "Sollarety",
        "smallImage": IMAGE_BASE + "os/{badge}.png",
        "smallText": "{os}",
        "smallTextUnknown": "Unknown system",
    },
}


def log(message):
    # journalctl ja carimba a hora; stdout sem buffer para o log nao atrasar.
    print(f"[presence] {message}", flush=True)


# ---------------------------------------------------------------------------
# Configuracao
# ---------------------------------------------------------------------------

def merge_states(user_states):
    """DEFAULT_STATES com o que vier de presence.json por cima, chave a chave.

    Assim o dono pode trocar so um texto sem copiar a secao inteira.
    """
    merged = {}
    for name, defaults in DEFAULT_STATES.items():
        section = dict(defaults)
        override = (user_states or {}).get(name)
        if override is not None:
            if not isinstance(override, dict):
                raise SystemExit(f"presence.json: states.{name} precisa ser um objeto")
            for key, value in override.items():
                if not key.startswith("//"):
                    section[key] = value
        merged[name] = section
    return merged


def validate_buttons(buttons, where):
    if len(buttons) > 2:
        raise SystemExit(f"presence.json: {where}: o Discord aceita no maximo 2 botoes")
    for button in buttons:
        if not isinstance(button, dict):
            raise SystemExit(f"presence.json: {where}: botao precisa ser um objeto: {button}")
        if not str(button.get("url", "")).startswith(("http://", "https://")):
            raise SystemExit(f"presence.json: {where}: botao sem URL http(s): {button}")
        if len(button.get("label", "")) > BUTTON_LABEL_MAX:
            raise SystemExit(f"presence.json: {where}: label de botao acima de 32 chars: {button}")


def validate_config(config):
    if not isinstance(config, dict):
        raise SystemExit("presence.json: o conteudo precisa ser um objeto JSON")

    if not config.get("applicationId"):
        raise SystemExit("presence.json: applicationId e obrigatorio")

    validate_buttons(config.get("buttons") or [], "buttons")

    states = config.get("states")
    if states is not None and not isinstance(states, dict):
        raise SystemExit("presence.json: states precisa ser um objeto")
    config["states"] = merge_states(states)

    for name, section in config["states"].items():
        for key in ("largeImage", "smallImage"):
            value = section.get(key)
            # URL publica e obrigatoria: chave de asset nao resolve (ver o
            # comentario de imagens em presence.json).
            if value and not str(value).startswith("https://"):
                raise SystemExit(f"presence.json: states.{name}.{key} precisa ser URL https")
        labels = section.get("repoButtonLabels")
        if labels is not None and (not isinstance(labels, list) or not all(isinstance(x, str) for x in labels)):
            raise SystemExit(f"presence.json: states.{name}.repoButtonLabels precisa ser lista de textos")

    try:
        poll = float(config.get("pollSeconds", POLL_SECONDS))
    except (TypeError, ValueError):
        raise SystemExit("presence.json: pollSeconds precisa ser numero")
    config["pollSeconds"] = max(POLL_MIN_SECONDS, poll)

    return config


def load_config():
    # Todo erro daqui vira SystemExit com mensagem legivel, nunca traceback: com
    # Restart=always no systemd, um arquivo ausente ou malformado viraria um
    # laco de crash a cada 15 s sem explicar o motivo.
    try:
        with open(CONFIG_PATH, "r", encoding="utf-8") as handle:
            config = json.load(handle)
    except OSError as exc:
        raise SystemExit(f"presence.json: nao consegui ler {CONFIG_PATH}: {exc}")
    except ValueError as exc:
        raise SystemExit(f"presence.json: JSON invalido: {exc}")

    return validate_config(config)


# ---------------------------------------------------------------------------
# IPC do Discord
# ---------------------------------------------------------------------------

def candidate_sockets():
    """Todos os caminhos onde o socket IPC do Discord pode estar.

    O Discord numera de 0 a 9 (varias instancias). Os subdiretorios cobrem as
    instalacoes em sandbox: o Flatpak e o Snap nao escrevem na raiz do
    XDG_RUNTIME_DIR, e sim dentro da propria caixa.
    """
    bases = []
    for env in ("XDG_RUNTIME_DIR", "TMPDIR", "TMP", "TEMP"):
        value = os.environ.get(env)
        if value:
            bases.append(value)
    bases.append("/tmp")

    subdirs = ("", "app/com.discordapp.Discord", "app/com.discordapp.DiscordCanary", "snap.discord")

    seen = set()
    for base in bases:
        for subdir in subdirs:
            directory = os.path.join(base, subdir) if subdir else base
            for index in range(10):
                path = os.path.join(directory, f"discord-ipc-{index}")
                if path not in seen:
                    seen.add(path)
                    yield path


def connect():
    for path in candidate_sockets():
        if not os.path.exists(path):
            continue
        try:
            sock = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
            # Sem timeout, um cliente Discord travado deixava o read_exactly
            # bloqueado para sempre e o processo so voltava a viver pelo
            # Restart=always da unit. Com timeout a sessao cai, o laco de
            # reconexao assume e o backoff funciona como foi desenhado.
            sock.settimeout(SOCKET_TIMEOUT)
            sock.connect(path)
            return sock, path
        except OSError:
            continue
    return None, None


def send(sock, opcode, payload):
    data = json.dumps(payload).encode("utf-8")
    sock.sendall(struct.pack("<II", opcode, len(data)) + data)


def recv(sock, idle_timeout=None):
    """Le um frame. idle_timeout limita so a espera ATE o primeiro byte.

    E o que faz o recv servir de relogio da varredura: sem frame dentro de
    idle_timeout sobe FrameIdle e o laco roda a deteccao.
    """
    header = read_exactly(sock, 8, idle_timeout)
    if header is None:
        return None, None
    opcode, length = struct.unpack("<II", header)
    # length vem do outro lado do socket e alimenta o read_exactly direto. Um
    # valor absurdo (ate 4 GiB, que e o teto de um <I) faria o processo tentar
    # acumular tudo em memoria. Nenhum frame legitimo do IPC chega perto disto.
    if length > MAX_FRAME_BYTES:
        log(f"frame de {length} bytes acima do teto de {MAX_FRAME_BYTES}; descartando a sessao")
        return None, None
    try:
        body = read_exactly(sock, length) if length else b""
    except FrameIdle:
        # O cabecalho ja foi consumido: silencio aqui e frame pela metade, nao
        # ociosidade. Tratar como FrameIdle dessincronizaria o fluxo.
        raise TimeoutError("corpo do frame nao chegou") from None
    if body is None:
        return None, None
    return opcode, json.loads(body.decode("utf-8")) if body else {}


class FrameIdle(OSError):
    """Nada chegou dentro do timeout, e estavamos no INICIO de um frame.

    Distinguir isto de um timeout no MEIO de um frame importa: no comeco nao ha
    nada em curso e da para simplesmente esperar de novo, enquanto no meio o
    fluxo ja perdeu o sincronismo e a sessao tem de cair.

    Herda de OSError de proposito: no laco de escuta ele e tratado
    explicitamente, mas se aparecer durante o handshake precisa cair no mesmo
    `except OSError` que ja derruba a sessao e devolve o controle ao laco de
    reconexao - e nao subir ate o main() e matar o processo.
    """


def read_exactly(sock, count, idle_timeout=None):
    # idle_timeout vale so ate o primeiro byte. Depois que algo chegou, o resto
    # do frame volta ao SOCKET_TIMEOUT: um tick curto (ex.: 0,2 s ate a proxima
    # varredura) nao pode derrubar um frame que so esta chegando devagar.
    if idle_timeout is not None:
        sock.settimeout(idle_timeout)
    chunks = b""
    try:
        while len(chunks) < count:
            try:
                chunk = sock.recv(count - len(chunks))
            except TimeoutError:
                if not chunks:
                    raise FrameIdle from None
                raise
            if not chunk:
                return None
            if not chunks and idle_timeout is not None:
                sock.settimeout(SOCKET_TIMEOUT)
            chunks += chunk
        return chunks
    finally:
        if idle_timeout is not None:
            try:
                sock.settimeout(SOCKET_TIMEOUT)
            except OSError:
                pass


# ---------------------------------------------------------------------------
# Utilitarios de deteccao
# ---------------------------------------------------------------------------

def run_cmd(args, timeout=CMD_TIMEOUT, env=None, cwd=None):
    """stdout do comando, ou None em qualquer falha. Nunca levanta."""
    full_env = None
    if env:
        full_env = dict(os.environ)
        full_env.update(env)
    try:
        result = subprocess.run(
            args,
            stdin=subprocess.DEVNULL,
            stdout=subprocess.PIPE,
            stderr=subprocess.DEVNULL,
            timeout=timeout,
            env=full_env,
            cwd=cwd,
            check=False,
        )
    except (OSError, subprocess.SubprocessError, ValueError):
        return None
    if result.returncode != 0:
        return None
    return result.stdout.decode("utf-8", "replace")


# git sem travas opcionais: o `git status` de fundo nao pode disputar o
# index.lock com o git que o dono estiver usando no mesmo repositorio.
GIT_ENV = {"GIT_OPTIONAL_LOCKS": "0", "GIT_TERMINAL_PROMPT": "0", "LC_ALL": "C"}


def git(workdir, *args, timeout=CMD_TIMEOUT):
    return run_cmd(["git", "-C", workdir, *args], timeout=timeout, env=GIT_ENV)


def boot_time():
    try:
        with open("/proc/stat", "r", encoding="ascii") as handle:
            for line in handle:
                if line.startswith("btime "):
                    return int(line.split()[1])
    except (OSError, ValueError):
        pass
    return None


CLOCK_TICKS = os.sysconf("SC_CLK_TCK") if hasattr(os, "sysconf") else 100


class Proc:
    __slots__ = ("pid", "ppid", "uid", "comm", "argv", "started")

    def __init__(self, pid, ppid, uid, comm, argv, started):
        self.pid = pid
        self.ppid = ppid
        self.uid = uid
        self.comm = comm
        self.argv = argv
        self.started = started

    def names(self):
        names = {self.comm.lower()}
        if self.argv:
            names.add(os.path.basename(self.argv[0]).lower())
        return names


def scan_processes(proc_root="/proc"):
    """Fotografia de /proc: pid, pai, dono, comm, argv e hora de inicio."""
    btime = boot_time()
    procs = []
    try:
        entries = os.listdir(proc_root)
    except OSError:
        return procs
    for entry in entries:
        if not entry.isdigit():
            continue
        base = os.path.join(proc_root, entry)
        try:
            uid = os.stat(base).st_uid
            with open(os.path.join(base, "stat"), "rb") as handle:
                stat = handle.read().decode("utf-8", "replace")
            with open(os.path.join(base, "cmdline"), "rb") as handle:
                cmdline = handle.read()
        except OSError:
            continue
        # O comm pode ter espacos e parenteses; o ultimo ")" fecha o campo.
        lpar, rpar = stat.find("("), stat.rfind(")")
        if lpar < 0 or rpar < 0:
            continue
        comm = stat[lpar + 1:rpar]
        fields = stat[rpar + 2:].split()
        try:
            ppid = int(fields[1])
            start_ticks = int(fields[19])
        except (IndexError, ValueError):
            continue
        started = btime + start_ticks / CLOCK_TICKS if btime else None
        argv = [a.decode("utf-8", "replace") for a in cmdline.split(b"\0") if a]
        procs.append(Proc(int(entry), ppid, uid, comm, argv, started))
    return procs


def proc_cwd(pid):
    try:
        return os.readlink(f"/proc/{pid}/cwd")
    except OSError:
        return None


# ---------------------------------------------------------------------------
# Sistema operacional das VMs
# ---------------------------------------------------------------------------

def osinfo_query_name(os_id):
    """Nome do SO pelo banco do libosinfo (`osinfo-query`), ou None."""
    out = run_cmd(["osinfo-query", "os", "--fields=name", f"id={os_id}"])
    if not out:
        return None
    lines = out.splitlines()
    for index, line in enumerate(lines):
        if set(line.strip()) == {"-"}:
            for value in lines[index + 1:]:
                if value.strip():
                    return value.strip()
            return None
    return None


def _version_suffix(version):
    if not version or version.lower() in ("rolling", "unknown", "latest"):
        return ""
    return " " + version


def _prettify_os(parts):
    if not parts:
        return None
    family = parts[0].replace("-", " ").replace("_", " ")
    family = family[:1].upper() + family[1:]
    version = parts[-1] if len(parts) > 1 else ""
    return (family + _version_suffix(version)).strip() or None


def os_from_libosinfo(os_id, query=osinfo_query_name):
    """Id do libosinfo (ex.: http://microsoft.com/win/11) -> {name, badge}.

    `query` e injetavel para os testes nao dependerem do osinfo-query.
    """
    if not os_id:
        return dict(UNKNOWN_OS)
    match = re.match(r"^[a-z]+://(?:www\.)?([^/]+)/(.*)$", os_id.strip(), re.IGNORECASE)
    if not match:
        return dict(UNKNOWN_OS)
    domain = match.group(1).lower()
    parts = [p for p in match.group(2).strip("/").split("/") if p]
    family = parts[0].lower() if parts else ""
    version = parts[-1] if len(parts) > 1 else ""

    def queried(prefix_strip=()):
        name = query(os_id) if query else None
        if name:
            for prefix in prefix_strip:
                if name.startswith(prefix):
                    name = name[len(prefix):]
        return name

    if domain == "microsoft.com":
        if family == "win" and version == "11":
            return {"name": "Windows 11", "badge": "windows11"}
        if family == "win" and version == "10":
            return {"name": "Windows 10", "badge": "windows10"}
        name = queried(("Microsoft ",)) or ("Windows" + _version_suffix(version))
        return {"name": name, "badge": "windows"}
    if domain == "gentoo.org":
        return {"name": "Gentoo", "badge": "gentoo"}
    if domain == "fedoraproject.org":
        if family == "fedora":
            suffix = " Rawhide" if version.lower() == "rawhide" else _version_suffix(version)
            return {"name": "Fedora" + suffix, "badge": "fedora"}
        return {"name": queried() or "Fedora", "badge": "fedora"}
    if domain == "ubuntu.com":
        return {"name": "Ubuntu" + _version_suffix(version), "badge": "ubuntu"}
    if domain == "debian.org":
        return {"name": "Debian" + _version_suffix(version), "badge": "debian"}
    if domain == "archlinux.org":
        return {"name": "Arch Linux", "badge": "arch"}
    if domain == "apple.com":
        return {"name": "macOS" + _version_suffix(version), "badge": "macos"}
    if domain == "freebsd.org":
        return {"name": "FreeBSD" + _version_suffix(version), "badge": "freebsd"}
    if domain == "libosinfo.org":
        if family == "linux":
            return {"name": "Linux", "badge": "linux"}
        return dict(UNKNOWN_OS)

    name = queried() or _prettify_os(parts)
    badge = "generic" if domain in NON_LINUX_DOMAINS else "linux"
    return {"name": name, "badge": badge}


# Pistas de SO pelo nome da VM, para quando nao ha libvirt nem metadado. O
# nome so e usado para ADIVINHAR o sistema, nunca aparece no Discord.
NAME_HINTS = [
    (re.compile(r"win(dows)?[ _-]?11"), {"name": "Windows 11", "badge": "windows11"}),
    (re.compile(r"win(dows)?[ _-]?10"), {"name": "Windows 10", "badge": "windows10"}),
    (re.compile(r"win"), {"name": "Windows", "badge": "windows"}),
    (re.compile(r"gentoo"), {"name": "Gentoo", "badge": "gentoo"}),
    (re.compile(r"fedora"), {"name": "Fedora", "badge": "fedora"}),
    (re.compile(r"ubuntu"), {"name": "Ubuntu", "badge": "ubuntu"}),
    (re.compile(r"debian"), {"name": "Debian", "badge": "debian"}),
    (re.compile(r"arch"), {"name": "Arch Linux", "badge": "arch"}),
    (re.compile(r"mac ?os|osx|darwin"), {"name": "macOS", "badge": "macos"}),
    (re.compile(r"freebsd"), {"name": "FreeBSD", "badge": "freebsd"}),
]


def os_from_hint(text):
    lowered = (text or "").lower()
    for pattern, info in NAME_HINTS:
        if pattern.search(lowered):
            return dict(info)
    return dict(UNKNOWN_OS)


def os_from_vmware_guest(guest_os):
    """guestOS do .vmx (ex.: windows11-64, windows9-64 = Windows 10)."""
    value = (guest_os or "").lower()
    if value.startswith("windows11"):
        return {"name": "Windows 11", "badge": "windows11"}
    if value.startswith("windows9"):
        return {"name": "Windows 10", "badge": "windows10"}
    if value.startswith(("windows", "win")):
        return {"name": "Windows", "badge": "windows"}
    if value.startswith("darwin"):
        return {"name": "macOS", "badge": "macos"}
    info = os_from_hint(value)
    if info["name"]:
        return info
    if "linux" in value:
        return {"name": "Linux", "badge": "linux"}
    return dict(UNKNOWN_OS)


def os_from_vbox_type(os_type):
    """ostype do VirtualBox (ex.: Windows11_64, Fedora_64, ArchLinux_64)."""
    value = (os_type or "").lower()
    info = os_from_hint(value)
    if info["name"]:
        return info
    if "linux" in value:
        return {"name": "Linux", "badge": "linux"}
    return dict(UNKNOWN_OS)


def libosinfo_id_from_xml(xml_text):
    try:
        root = ElementTree.fromstring(xml_text)
    except ElementTree.ParseError:
        return None
    for element in root.iter():
        if element.tag.endswith("}os") and "libosinfo" in element.tag and element.get("id"):
            return element.get("id")
    return None


def qemu_guest_name(argv):
    for index, arg in enumerate(argv):
        if arg == "-name" and index + 1 < len(argv):
            value = argv[index + 1]
            for piece in value.split(","):
                if piece.startswith("guest="):
                    return piece[len("guest="):]
            return value.split(",")[0]
    return None


def qemu_hint_windows(argv):
    # Enlightenments do Hyper-V no -cpu (hv_relaxed, hv-vapic...) so se ligam
    # para convidado Windows: boa pista quando nada mais diz o sistema.
    for index, arg in enumerate(argv[:-1]):
        if arg == "-cpu" and re.search(r"\bhv[_-]", argv[index + 1]):
            return True
    return False


# ---------------------------------------------------------------------------
# Repositorios
# ---------------------------------------------------------------------------

def parse_github_remote(url):
    """URL do remote -> (owner, name) se for do GitHub, senao None.

    Cobre git@github.com:o/n(.git), ssh://git@github.com[:porta]/o/n(.git),
    https://[usuario@]github.com/o/n(.git)(/) e git://github.com/o/n.
    """
    if not url:
        return None
    match = re.match(
        r"^(?:(?:ssh|git|https?|git\+ssh)://)?(?:[^@/\s]+@)?(?:www\.)?github\.com(?::\d+)?[:/]"
        r"([A-Za-z0-9](?:[A-Za-z0-9-]{0,38}))/([A-Za-z0-9._-]+?)(?:\.git)?/*$",
        url.strip(),
        re.IGNORECASE,
    )
    if not match:
        return None
    owner, name = match.group(1), match.group(2)
    if name in (".", ".."):
        return None
    return owner, name


def language_for_path(path):
    _, ext = os.path.splitext(path.lower())
    return EXTENSION_LANGUAGES.get(ext)


def majority_language(files):
    """files = [(caminho, mtime)]. Linguagem com mais arquivos; empate vai
    para a que tiver o arquivo mais recente. None se nenhuma for conhecida."""
    counts = {}
    latest = {}
    for path, mtime in files:
        lang = language_for_path(path)
        if not lang:
            continue
        counts[lang] = counts.get(lang, 0) + 1
        latest[lang] = max(latest.get(lang, 0), mtime or 0)
    if not counts:
        return None
    return max(counts, key=lambda lang: (counts[lang], latest[lang]))


def dominant_language(paths):
    """Linguagem dominante pelos arquivos versionados. Markdown so ganha se
    nao houver mais nada: documentacao nao e o que se esta programando."""
    counts = {}
    for path in paths:
        lang = language_for_path(path)
        if lang:
            counts[lang] = counts.get(lang, 0) + 1
    if not counts:
        return None
    code = {lang: n for lang, n in counts.items() if lang != "markdown"}
    pool = code or counts
    return max(pool, key=lambda lang: (pool[lang], lang))


def parse_status_z(output):
    """Saida de `git status --porcelain -z` -> lista de caminhos."""
    paths = []
    tokens = output.split("\0")
    index = 0
    while index < len(tokens):
        token = tokens[index]
        index += 1
        if len(token) < 4:
            continue
        xy, path = token[:2], token[3:]
        if "R" in xy or "C" in xy:
            # Renomeacao/copia: o proximo token e o caminho de origem.
            index += 1
        paths.append(path)
    return paths


def find_git_root(path, home):
    """Sobe a partir de path ate achar .git. Nunca considera a propria home
    (um repositorio de dotfiles ali engoliria todo diretorio)."""
    current = path
    while current and current != home and current != "/":
        if os.path.exists(os.path.join(current, ".git")):
            return current
        parent = os.path.dirname(current)
        if parent == current:
            break
        current = parent
    return None


def http_get_json(url, timeout=CMD_TIMEOUT):
    request = urllib.request.Request(url, headers={
        "Accept": "application/vnd.github+json",
        "User-Agent": "daeese-presence",
        "X-GitHub-Api-Version": "2022-11-28",
    })
    with urllib.request.urlopen(request, timeout=timeout) as response:
        return json.loads(response.read(1 << 20).decode("utf-8"))


# ---------------------------------------------------------------------------
# Detector
# ---------------------------------------------------------------------------

class Detector:
    """Encontra VMs e programacao em andamento. Toda falha vira "nada"."""

    def __init__(self, config, clock=time.time, home=None):
        self.config = config
        self.clock = clock
        self.home = os.path.realpath(home or os.path.expanduser("~"))
        self.uid = os.getuid()
        self._os_cache = {}
        self._lang_cache = {}
        self._remote_cache = {}
        self._main_root_cache = {}
        self._github_cache = {}
        self._warned = set()

    def warn_once(self, key, message):
        if key not in self._warned:
            self._warned.add(key)
            log(message)

    # -- VMs ---------------------------------------------------------------

    def detect_vms(self, procs):
        """Lista de VMs rodando: {key, os, started, detector}."""
        qemu = [p for p in procs if p.comm.startswith("qemu-system") or
                (p.argv and os.path.basename(p.argv[0]).startswith("qemu-system"))]
        vms = []
        covered = set()

        # Toda VM do driver qemu do libvirt e um processo qemu-system. Sem
        # nenhum, nao ha por que acordar o virtqemud a cada 15 s.
        if qemu:
            by_guest = {}
            for proc in qemu:
                guest = qemu_guest_name(proc.argv)
                if guest:
                    by_guest.setdefault(guest, proc)
            for uri in ("qemu:///system", "qemu:///session"):
                for name in self.libvirt_running(uri):
                    proc = by_guest.get(name)
                    if proc is not None:
                        covered.add(proc.pid)
                    vms.append({
                        "key": f"libvirt:{uri}:{name}",
                        "os": self.libvirt_os(uri, name),
                        "started": proc.started if proc else None,
                        "detector": "libvirt",
                    })

            for proc in qemu:
                if proc.pid in covered:
                    continue
                guest = qemu_guest_name(proc.argv) or ""
                info = os_from_hint(guest)
                if not info["name"] and qemu_hint_windows(proc.argv):
                    info = {"name": "Windows", "badge": "windows"}
                vms.append({
                    "key": f"qemu:{proc.pid}",
                    "os": info,
                    "started": proc.started,
                    "detector": "proc-qemu",
                })

        for proc in procs:
            names = proc.names()
            if names & VBOX_NAMES:
                vm_name = None
                for index, arg in enumerate(proc.argv):
                    if arg in ("--startvm", "-startvm", "-s") and index + 1 < len(proc.argv):
                        vm_name = proc.argv[index + 1]
                        break
                vms.append({
                    "key": f"vbox:{proc.pid}",
                    "os": self.vbox_os(vm_name),
                    "started": proc.started,
                    "detector": "virtualbox",
                })
            elif names & VMWARE_NAMES:
                vmx = next((a for a in reversed(proc.argv) if a.lower().endswith(".vmx")), None)
                vms.append({
                    "key": f"vmware:{proc.pid}",
                    "os": self.vmware_os(vmx),
                    "started": proc.started,
                    "detector": "vmware",
                })
        return vms

    def libvirt_running(self, uri):
        # LIBVIRT_AUTOSTART=0: sem isto, consultar qemu:///session sobe um
        # virtqemud de sessao so para responder que nao ha nada.
        out = run_cmd(["virsh", "-c", uri, "list", "--state-running", "--name"],
                      env={"LIBVIRT_AUTOSTART": "0", "LC_ALL": "C"})
        if not out:
            return []
        return [line.strip() for line in out.splitlines() if line.strip()]

    def libvirt_os(self, uri, name):
        key = (uri, name)
        cached = self._os_cache.get(key)
        now = self.clock()
        if cached and cached[0] > now:
            return cached[1]
        xml = run_cmd(["virsh", "-c", uri, "dumpxml", name],
                      env={"LIBVIRT_AUTOSTART": "0", "LC_ALL": "C"})
        os_id = libosinfo_id_from_xml(xml) if xml else None
        info = os_from_libosinfo(os_id) if os_id else os_from_hint(name)
        # Falha de leitura (xml None) fica pouco tempo no cache: pode ter sido
        # so o daemon reiniciando.
        self._os_cache[key] = (now + (OS_CACHE_TTL if xml else 60), info)
        return info

    def vbox_os(self, vm_name):
        if not vm_name:
            return dict(UNKNOWN_OS)
        key = ("vbox", vm_name)
        cached = self._os_cache.get(key)
        now = self.clock()
        if cached and cached[0] > now:
            return cached[1]
        info = None
        out = run_cmd(["VBoxManage", "showvminfo", vm_name, "--machinereadable"])
        if out:
            match = re.search(r'^ostype="([^"]*)"', out, re.MULTILINE)
            if match:
                info = os_from_vbox_type(match.group(1))
        if not info or not info["name"]:
            info = os_from_hint(vm_name)
        self._os_cache[key] = (now + OS_CACHE_TTL, info)
        return info

    def vmware_os(self, vmx):
        if not vmx:
            return dict(UNKNOWN_OS)
        try:
            with open(vmx, "r", encoding="utf-8", errors="replace") as handle:
                text = handle.read(256 * 1024)
        except OSError:
            return os_from_hint(os.path.basename(vmx))
        match = re.search(r'^\s*guestOS\s*=\s*"([^"]*)"', text, re.MULTILINE | re.IGNORECASE)
        if match:
            info = os_from_vmware_guest(match.group(1))
            if info["name"]:
                return info
        return os_from_hint(os.path.basename(vmx))

    # -- Programacao -------------------------------------------------------

    @staticmethod
    def is_coding_process(proc):
        names = proc.names()
        if names & EDITOR_NAMES or names & LSP_NAMES:
            return True
        # node/python/dotnet rodando um servidor de linguagem: o nome util
        # esta nos primeiros argumentos.
        for arg in proc.argv[1:4]:
            base = os.path.basename(arg).lower()
            for ext in (".js", ".mjs", ".cjs", ".py", ".dll", ".exe"):
                if base.endswith(ext):
                    base = base[:-len(ext)]
            if base in LSP_NAMES:
                return True
        # IDEs da JetBrains rodam como java com propriedades -Didea.*.
        if proc.comm in ("java", "java.exe") and any(
                a.startswith("-Didea.") or "jetbrains" in a.lower() for a in proc.argv[:60]):
            return True
        return False

    def candidate_dirs(self, procs):
        """cwds dos editores/agentes e de todos os descendentes deles."""
        mine = [p for p in procs if p.uid == self.uid]
        children = {}
        for proc in mine:
            children.setdefault(proc.ppid, []).append(proc)
        roots = [p for p in mine if self.is_coding_process(p)]

        seen = set()
        stack = list(roots)
        while stack:
            proc = stack.pop()
            if proc.pid in seen:
                continue
            seen.add(proc.pid)
            stack.extend(children.get(proc.pid, ()))

        cache_dir = os.path.join(self.home, ".cache")
        dirs = set()
        for pid in seen:
            cwd = proc_cwd(pid)
            if not cwd or cwd.endswith(" (deleted)"):
                continue
            cwd = os.path.realpath(cwd)
            if cwd == self.home or not cwd.startswith(self.home + os.sep):
                continue
            if cwd == cache_dir or cwd.startswith(cache_dir + os.sep):
                continue
            dirs.add(cwd)
        return dirs

    def candidate_repos(self, dirs):
        """Worktrees git a partir dos cwds.

        Um cwd fora de qualquer repositorio mas com repositorios logo abaixo
        (ex.: ~/daeesewrkspc, de onde o Claude Code roda) entra como pasta de
        trabalho: os filhos diretos com .git viram candidatos. O filtro de
        "mudou recentemente" decide qual deles esta de fato sendo mexido.
        """
        repos = set()
        for cwd in dirs:
            root = find_git_root(cwd, self.home)
            if root:
                repos.add(root)
                continue
            try:
                with os.scandir(cwd) as entries:
                    for count, entry in enumerate(entries):
                        if count >= WORKSPACE_MAX_ENTRIES:
                            break
                        try:
                            if entry.is_dir(follow_symlinks=False) and \
                                    os.path.exists(os.path.join(entry.path, ".git")):
                                repos.add(entry.path)
                        except OSError:
                            continue
            except OSError:
                continue
        return repos

    def main_root(self, workdir):
        """Raiz do repositorio principal (worktrees apontam para ele)."""
        if workdir in self._main_root_cache:
            return self._main_root_cache[workdir]
        result = workdir
        if os.path.isfile(os.path.join(workdir, ".git")):
            out = git(workdir, "rev-parse", "--path-format=absolute", "--git-common-dir")
            common = out.strip() if out else ""
            # So worktree tem common dir terminando em .git; submodulo aponta
            # para .git/modules/<x> e continua sendo o proprio repositorio.
            if common and os.path.basename(common.rstrip("/")) == ".git":
                result = os.path.dirname(common.rstrip("/"))
        self._main_root_cache[workdir] = result
        return result

    def recent_changes(self, workdir, now, window):
        """(ultima_mudanca, [(arquivo, mtime)]) dentro da janela, ou None."""
        files = []
        out = git(workdir, "status", "--porcelain=v1", "-z", "--untracked-files=all", "--no-renames")
        if out is None:
            return None
        for path in parse_status_z(out)[:STATUS_MAX_ENTRIES]:
            try:
                mtime = os.lstat(os.path.join(workdir, path)).st_mtime
            except OSError:
                continue  # apagado: sem mtime para medir
            if now - window <= mtime <= now + 60:
                files.append((path, min(mtime, now)))

        head = git(workdir, "log", "-1", "--format=%ct")
        try:
            head_time = int(head.strip()) if head and head.strip() else None
        except ValueError:
            head_time = None
        if head_time and now - window <= head_time <= now + 60:
            seen = {p for p, _ in files}
            committed = git(workdir, "diff-tree", "--no-commit-id", "--name-only", "-r", "-z", "--root", "HEAD")
            for path in (committed or "").split("\0"):
                if path and path not in seen:
                    files.append((path, head_time))
            if not files:
                files.append(("", head_time))

        if not files:
            return None
        return max(m for _, m in files), files

    def repo_language(self, root, workdir):
        now = self.clock()
        cached = self._lang_cache.get(root)
        if cached and cached[0] > now:
            return cached[1]
        out = git(workdir, "ls-files", "-z")
        lang = dominant_language(out.split("\0")) if out else None
        self._lang_cache[root] = (now + LANG_CACHE_TTL, lang)
        return lang

    def origin_url(self, root, workdir):
        now = self.clock()
        cached = self._remote_cache.get(root)
        if cached and cached[0] > now:
            return cached[1]
        out = git(workdir, "remote", "get-url", "origin")
        url = out.strip() if out else None
        self._remote_cache[root] = (now + REMOTE_CACHE_TTL, url)
        return url

    def github_public(self, owner, name, fetch=None):
        """{name, url} se o repositorio for PUBLICO no GitHub, senao None.

        Decisao do dono: qualquer duvida (privado, 404, rede, rate limit) e
        tratada como privado. So o 'definitivo' fica 6 h no cache; erro de
        rede fica 15 min para nao esconder um repo publico por horas.
        """
        key = (owner.lower(), name.lower())
        now = self.clock()
        cached = self._github_cache.get(key)
        if cached and cached[0] > now:
            return cached[1]
        fetch = fetch or http_get_json
        result, ttl = None, GITHUB_TTL
        try:
            data = fetch(f"https://api.github.com/repos/{owner}/{name}")
            if isinstance(data, dict) and data.get("private") is False and data.get("name"):
                url = data.get("html_url")
                if not (isinstance(url, str) and url.startswith("https://github.com/")):
                    url = f"https://github.com/{owner}/{data['name']}"
                result = {"name": str(data["name"]), "url": url}
        except urllib.error.HTTPError as exc:
            exc.close()
            if exc.code != 404:
                ttl = GITHUB_ERROR_TTL
                self.warn_once(f"gh{exc.code}", f"GitHub respondeu {exc.code}; repositorio tratado como privado")
        except (OSError, ValueError) as exc:
            ttl = GITHUB_ERROR_TTL
            self.warn_once("ghnet", f"GitHub inacessivel ({type(exc).__name__}); repositorio tratado como privado")
        self._github_cache[key] = (now + ttl, result)
        return result

    def detect_coding(self, procs):
        section = self.config["states"]["coding"]
        try:
            window = float(section.get("recentMinutes", RECENT_MINUTES)) * 60
        except (TypeError, ValueError):
            window = RECENT_MINUTES * 60
        now = self.clock()

        dirs = self.candidate_dirs(procs)
        repos = self.candidate_repos(dirs)
        best = None
        for workdir in sorted(repos):
            change = self.recent_changes(workdir, now, window)
            if change and (best is None or change[0] > best[1]):
                best = (workdir, change[0], change[1])
        if best is None:
            return None

        workdir, last_change, files = best
        root = self.main_root(workdir)
        lang = majority_language(files) or self.repo_language(root, workdir) or "generic"
        if lang not in LANG_BADGES:
            lang = "generic"

        public = None
        remote = parse_github_remote(self.origin_url(root, workdir))
        if remote:
            public = self.github_public(*remote)

        return {
            "repo_key": root,
            "workdir": workdir,
            "language": LANGUAGE_NAMES.get(lang, "code"),
            "badge": lang,
            "public": public,
            "last_change": last_change,
            "candidates": len(repos),
            "detector": "process-scan+git",
        }


# ---------------------------------------------------------------------------
# Estado e atividade
# ---------------------------------------------------------------------------

class Timeline:
    """Guarda o 'desde quando' de cada estado entre as varreduras.

    O timestamp so pode mudar quando o estado muda de verdade; se ele
    variasse a cada tick, a atividade nunca seria igual a anterior e cada
    varredura viraria um SET_ACTIVITY.
    """

    def __init__(self):
        self.vm_seen = {}
        self.coding_key = None
        self.coding_since = None

    def update(self, vms, coding, now):
        current = {vm["key"] for vm in vms}
        for vm in vms:
            if vm["key"] not in self.vm_seen:
                started = vm.get("started")
                # Inicio do processo da VM quando conhecido: reiniciar o
                # servico de presenca nao zera o contador de uma VM que ja
                # estava rodando.
                self.vm_seen[vm["key"]] = min(now, started) if started else now
        for key in list(self.vm_seen):
            if key not in current:
                del self.vm_seen[key]

        key = None
        if coding:
            key = ("vm" if vms else "coding", coding["repo_key"])
        if key != self.coding_key:
            self.coding_key = key
            self.coding_since = now if key else None

        return dict(self.vm_seen), self.coding_since


def select_state(vms, coding, timeline, now):
    """Escolhe o estado. VM tem prioridade e carrega a programacao junto."""
    vm_seen, coding_since = timeline.update(vms, coding, now)
    if vms:
        ordered = sorted(vms, key=lambda vm: (vm_seen.get(vm["key"], now), vm["key"]))
        primary = ordered[0]
        return {
            "kind": "vm",
            "vm": primary,
            "vm_count": len(vms),
            "coding": coding,
            "since": vm_seen.get(primary["key"], now),
            "detector": primary["detector"] + ("+coding" if coding else ""),
        }
    if coding:
        return {
            "kind": "coding",
            "vm": None,
            "vm_count": 0,
            "coding": coding,
            "since": coding_since,
            "detector": coding["detector"],
        }
    return {"kind": "idle", "vm": None, "vm_count": 0, "coding": None, "since": None, "detector": None}


def fill(template, fallback, **values):
    """Formata um texto do presence.json sem nunca levantar."""
    for candidate in (template, fallback):
        if not isinstance(candidate, str):
            continue
        try:
            return candidate.format(**values)
        except (KeyError, IndexError, ValueError, AttributeError):
            continue
    return ""


def clamp_text(text):
    text = (text or "").strip()
    if len(text) > TEXT_MAX:
        text = text[:TEXT_MAX - 1].rstrip() + "…"
    # O Discord recusa details/state com menos de 2 caracteres.
    return text if len(text) >= 2 else None


def build_idle_activity(config, started_at):
    assets = {}
    if config.get("largeImage"):
        assets["large_image"] = config["largeImage"]
        if config.get("largeText"):
            assets["large_text"] = config["largeText"]
    if config.get("smallImage"):
        assets["small_image"] = config["smallImage"]
        if config.get("smallText"):
            assets["small_text"] = config["smallText"]

    activity = {}
    # 0 Playing, 2 Listening, 3 Watching, 5 Competing. Importa mais do que
    # parece: o cliente Discord mantem um unico card do tipo "Playing", e um
    # jogo detectado (Roblox, por exemplo) ganha dele. Com um tipo diferente o
    # cartao convive com o jogo em vez de ser descartado.
    activity["type"] = int(config.get("activityType", 0))
    if config.get("details"):
        activity["details"] = config["details"]
    if config.get("state"):
        activity["state"] = config["state"]
    if assets:
        activity["assets"] = assets
    if config.get("buttons"):
        activity["buttons"] = config["buttons"]
    if config.get("showElapsed", True):
        # O Discord espera milissegundos. started_at e fixado no inicio do
        # processo, e nao a cada reconexao, para o contador nao zerar toda vez
        # que o cliente Discord reinicia.
        activity["timestamps"] = {"start": int(started_at * 1000)}

    return activity


def repo_button(section, repo):
    for template in section.get("repoButtonLabels") or DEFAULT_STATES["coding"]["repoButtonLabels"]:
        label = fill(template, None, repo=repo["name"]).strip()
        if 0 < len(label) <= BUTTON_LABEL_MAX:
            return {"label": label, "url": repo["url"]}
    return None


def dynamic_buttons(config, coding):
    idle_buttons = list(config.get("buttons") or [])
    public = coding.get("public") if coding else None
    if public:
        button = repo_button(config["states"]["coding"], public)
        if button:
            return (idle_buttons[:1] + [button])[:2]
    return idle_buttons[:2]


def build_activity(config, snapshot, started_at):
    kind = snapshot["kind"]
    if kind == "idle":
        return build_idle_activity(config, started_at)

    coding = snapshot.get("coding")
    language = coding["language"] if coding else None
    public = coding.get("public") if coding else None
    repo = public["name"] if public else None

    if kind == "coding":
        section = config["states"]["coding"]
        defaults = DEFAULT_STATES["coding"]
        values = {"language": language, "repo": repo or "", "badge": coding["badge"]}
        details = fill(section.get("details"), defaults["details"], **values)
        if repo:
            state = fill(section.get("state"), defaults["state"], **values)
        else:
            state = fill(section.get("statePrivate"), defaults["statePrivate"], **values)
        large_image = fill(section.get("largeImage"), defaults["largeImage"], **values)
        large_text = fill(section.get("largeText"), defaults["largeText"], **values)
        small_image = fill(section.get("smallImage"), defaults["smallImage"], **values)
        small_text = fill(section.get("smallText"), defaults["smallText"], **values)
    else:
        section = config["states"]["vm"]
        defaults = DEFAULT_STATES["vm"]
        vm_os = snapshot["vm"]["os"]
        os_name = vm_os.get("name")
        badge = vm_os.get("badge") if vm_os.get("badge") in OS_BADGES else "generic"
        count = snapshot.get("vm_count", 1)
        values = {
            "os": os_name or "", "badge": badge, "count": count,
            "language": language or "", "repo": repo or "",
        }
        if os_name:
            details = fill(section.get("details"), defaults["details"], **values)
            small_text = fill(section.get("smallText"), defaults["smallText"], **values)
        else:
            details = fill(section.get("detailsUnknown"), defaults["detailsUnknown"], **values)
            small_text = fill(section.get("smallTextUnknown"), defaults["smallTextUnknown"], **values)
        if coding and repo:
            state = fill(section.get("stateCoding"), defaults["stateCoding"], **values)
        elif coding:
            state = fill(section.get("stateCodingPrivate"), defaults["stateCodingPrivate"], **values)
        elif count > 1:
            state = fill(section.get("stateMany"), defaults["stateMany"], **values)
        elif os_name:
            state = fill(section.get("state"), defaults["state"], **values)
        else:
            state = fill(section.get("stateUnknown"), defaults["stateUnknown"], **values)
        large_image = fill(section.get("largeImage"), defaults["largeImage"], **values)
        large_text = fill(section.get("largeText"), defaults["largeText"], **values)
        small_image = fill(section.get("smallImage"), defaults["smallImage"], **values)

    activity = {
        "type": int(section.get("activityType", config.get("activityType", 3))),
        # 2 = DETAILS: a lista de membros mostra a linha de details ("Orbiting
        # Windows 11") no lugar do nome da aplicacao ("Watching Sollarety").
        "status_display_type": int(section.get("statusDisplayType", 2)),
    }
    details, state = clamp_text(details), clamp_text(state)
    if details:
        activity["details"] = details
    if state:
        activity["state"] = state

    assets = {}
    if large_image.startswith("https://"):
        assets["large_image"] = large_image
        if clamp_text(large_text):
            assets["large_text"] = clamp_text(large_text)
    if small_image.startswith("https://"):
        assets["small_image"] = small_image
        if clamp_text(small_text):
            assets["small_text"] = clamp_text(small_text)
    if assets:
        activity["assets"] = assets

    since = snapshot.get("since")
    if since:
        activity["timestamps"] = {"start": int(since * 1000)}

    buttons = dynamic_buttons(config, coding)
    if buttons:
        activity["buttons"] = buttons
    return activity


def signature(activity):
    return json.dumps(activity, sort_keys=True, separators=(",", ":"))


def describe(snapshot):
    """Linha de log sem caminhos nem nomes de VM."""
    if snapshot.get("fallback"):
        return "idle (a atividade de " + describe(dict(snapshot, fallback=False)) + " foi recusada)"
    coding = snapshot.get("coding")
    code = ""
    if coding:
        where = coding["public"]["name"] if coding.get("public") else "repo privado"
        code = f"{coding['language']} em {where}"
    if snapshot["kind"] == "vm":
        name = snapshot["vm"]["os"].get("name") or "SO desconhecido"
        extra = f" + {code}" if code else ""
        return f"vm ({snapshot['vm_count']}x, {name}){extra}"
    if snapshot["kind"] == "coding":
        return f"coding ({code})"
    return "idle"


class PresenceEngine:
    def __init__(self, config, started_at, detector=None, clock=time.time):
        self.config = config
        self.started_at = started_at
        self.clock = clock
        self.detector = detector or Detector(config, clock=clock)
        self.timeline = Timeline()
        # Assinaturas que o Discord recusou: nao reenviar a mesma coisa a cada
        # tick, cair para o cartao idle ate a atividade mudar.
        self.rejected = set()
        self.last_vms = []

    def idle_activity(self):
        return build_idle_activity(self.config, self.started_at)

    def detect(self):
        states = self.config["states"]
        vms, coding = [], None
        procs = []
        try:
            procs = scan_processes()
        except Exception as exc:  # noqa: BLE001 - deteccao nunca derruba o servico
            self.detector.warn_once("scan", f"varredura de /proc falhou: {exc}")
        if states["vm"].get("enabled", True):
            try:
                vms = self.detector.detect_vms(procs)
            except Exception as exc:  # noqa: BLE001
                log(f"deteccao de VM falhou: {type(exc).__name__}: {exc}")
                vms = []
        if states["coding"].get("enabled", True):
            try:
                coding = self.detector.detect_coding(procs)
            except Exception as exc:  # noqa: BLE001
                log(f"deteccao de codigo falhou: {type(exc).__name__}: {exc}")
                coding = None
        self.last_vms = vms
        return vms, coding

    def compute(self):
        vms, coding = self.detect()
        snapshot = select_state(vms, coding, self.timeline, self.clock())
        try:
            activity = build_activity(self.config, snapshot, self.started_at)
        except Exception as exc:  # noqa: BLE001
            log(f"montagem da atividade falhou: {type(exc).__name__}: {exc}")
            activity = self.idle_activity()
        if signature(activity) in self.rejected:
            activity = self.idle_activity()
            snapshot = dict(snapshot, fallback=True)
        return activity, snapshot


# ---------------------------------------------------------------------------
# Sessao e laco principal
# ---------------------------------------------------------------------------

def run_session(config, engine):
    """Uma conexao, do handshake ate cair. Retorna quando o socket morre.

    O recv com timeout faz as vezes de relogio: espera frames ate a proxima
    varredura; o FrameIdle (silencio no inicio de um frame) e o caminho normal
    de "hora de varrer de novo". PINGs continuam respondidos no meio disso.
    """
    sock, path = connect()
    if sock is None:
        return False

    poll = config["pollSeconds"]
    idle_sig = signature(engine.idle_activity())
    try:
        send(sock, OP_HANDSHAKE, {"v": 1, "client_id": str(config["applicationId"])})
        opcode, payload = recv(sock)
        if opcode is None:
            return False
        if opcode == OP_CLOSE or payload.get("evt") == "ERROR":
            log(f"handshake recusado: {payload.get('data') or payload}")
            return False

        user = (payload.get("data") or {}).get("user") or {}
        log(f"conectado em {path} como {user.get('username', '?')}")

        pending = {}       # nonce -> assinatura enviada
        current_sig = None  # o que o Discord esta mostrando (ou vai mostrar)
        last_send = 0.0
        next_tick = 0.0
        confirmed = False

        while True:
            now = time.monotonic()
            if now >= next_tick:
                activity, snapshot = engine.compute()
                sig = signature(activity)
                if sig != current_sig and now - last_send >= MIN_SEND_INTERVAL:
                    nonce = str(uuid.uuid4())
                    send(sock, OP_FRAME, {
                        "cmd": "SET_ACTIVITY",
                        "args": {"pid": os.getpid(), "activity": activity},
                        "nonce": nonce,
                    })
                    pending[nonce] = sig
                    current_sig = sig
                    last_send = time.monotonic()
                    log(f"atividade: {describe(snapshot)}")
                # A partir do fim da varredura: o git/virsh/GitHub demorando
                # nao encurta o intervalo seguinte.
                next_tick = time.monotonic() + poll

            try:
                opcode, payload = recv(sock, idle_timeout=max(0.05, next_tick - time.monotonic()))
            except FrameIdle:
                # Silencio do Discord e o estado NORMAL aqui: depois do
                # SET_ACTIVITY so chegam PINGs esporadicos e as respostas.
                # Acordar e ir varrer e o comportamento certo, e nao derrubar
                # uma sessao saudavel.
                continue
            if opcode is None:
                log("socket encerrado pelo Discord")
                return True
            if opcode == OP_PING:
                send(sock, OP_PONG, payload)
            elif opcode == OP_CLOSE:
                log(f"Discord fechou a sessao: {payload}")
                return False
            elif opcode == OP_FRAME:
                sent = pending.pop(payload.get("nonce"), None)
                if sent is None:
                    continue
                if payload.get("evt") == "ERROR":
                    log(f"SET_ACTIVITY recusado: {payload.get('data')}")
                    if sent == idle_sig:
                        # O cartao fixo e config do dono: recusado, nao ha
                        # fallback. Mesmo comportamento de antes - backoff.
                        return False
                    # Atividade dinamica recusada: marca e volta ao idle no
                    # proximo tick (respeitando o piso entre envios).
                    engine.rejected.add(sent)
                    if current_sig == sent:
                        current_sig = None
                    next_tick = min(next_tick, last_send + MIN_SEND_INTERVAL)
                elif not confirmed:
                    confirmed = True
                    log("presenca definida")
    except (OSError, ValueError, AttributeError, KeyError) as exc:
        # O caso mais comum de todos -- fechar o cliente do Discord no meio da
        # sessao -- levanta OSError/BrokenPipeError dentro de send()/recv().
        # Sem este except a excecao subia por main() e matava o processo com
        # traceback, entao o backoff exponencial logo abaixo NUNCA rodava: quem
        # trazia o servico de volta era so o Restart=always do systemd.
        # ValueError cobre o json.loads de um frame truncado; AttributeError e
        # KeyError, um frame com forma inesperada.
        log(f"sessao caiu: {type(exc).__name__}: {exc}")
        return False
    finally:
        try:
            sock.close()
        except OSError:
            pass


def dry_run(config, started_at):
    """--once / --dry-run: calcula uma vez e imprime, sem tocar no Discord."""
    engine = PresenceEngine(config, started_at)
    activity, snapshot = engine.compute()
    coding = snapshot.get("coding")
    report = {
        "state": snapshot["kind"],
        "detector": snapshot["detector"],
        # Sem o nome das VMs: so o que o cartao usaria.
        "vms": [
            {"os": vm["os"].get("name"), "badge": vm["os"].get("badge"), "detector": vm["detector"]}
            for vm in engine.last_vms
        ],
        "coding": None if not coding else {
            "local_workdir": coding["workdir"],
            "language": coding["language"],
            "badge": coding["badge"],
            "public_repo": coding["public"]["name"] if coding.get("public") else None,
            "last_change_seconds_ago": int(time.time() - coding["last_change"]),
            "candidate_repos": coding["candidates"],
        },
        "activity": activity,
    }
    print(json.dumps(report, indent=2, ensure_ascii=False))


def main(argv=None):
    argv = sys.argv[1:] if argv is None else argv
    unknown = [a for a in argv if a not in ("--once", "--dry-run")]
    if unknown:
        raise SystemExit(f"uso: daeese_presence.py [--once | --dry-run]  (desconhecido: {' '.join(unknown)})")

    config = load_config()
    started_at = time.time()

    if argv:
        dry_run(config, started_at)
        return

    def stop(signum, _frame):
        # Fechar o processo ja limpa a presenca: o Discord a descarta quando o
        # socket cai. Nao ha o que "desfazer" no servidor.
        #
        # sys.exit levanta SystemExit e desenrola a pilha na hora, entao nao
        # existe "avisar o laco para parar": a flag booleana que morava aqui
        # nunca chegava a ser lida.
        log(f"sinal {signum} recebido, encerrando")
        sys.exit(0)

    signal.signal(signal.SIGTERM, stop)
    signal.signal(signal.SIGINT, stop)

    # Um motor para a vida do processo: caches (GitHub, SO das VMs) e o
    # "desde quando" de cada estado sobrevivem a reconexoes com o Discord.
    engine = PresenceEngine(config, started_at)

    delay = RECONNECT_MIN
    while True:
        connected = run_session(config, engine)
        if connected:
            # Fechamento limpo do Discord: reconecta na hora. Antes o time.sleep
            # abaixo rodava tambem neste caminho, deixando a presenca ausente por
            # RECONNECT_MIN segundos apos cada reconexao normal.
            delay = RECONNECT_MIN
            continue

        delay = min(delay * 2, RECONNECT_MAX)
        log(f"Discord indisponivel, nova tentativa em {delay}s")
        time.sleep(delay)


if __name__ == "__main__":
    main()
