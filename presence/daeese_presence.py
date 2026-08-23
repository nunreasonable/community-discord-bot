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

Sem dependencias externas de proposito: o protocolo IPC e simples e a maquina
nao precisa de um venv so para isto.
"""

import json
import os
import signal
import socket
import struct
import sys
import time
import uuid

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


def log(message):
    # journalctl ja carimba a hora; stdout sem buffer para o log nao atrasar.
    print(f"[presence] {message}", flush=True)


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

    if not isinstance(config, dict):
        raise SystemExit("presence.json: o conteudo precisa ser um objeto JSON")

    if not config.get("applicationId"):
        raise SystemExit("presence.json: applicationId e obrigatorio")

    buttons = config.get("buttons") or []
    if len(buttons) > 2:
        raise SystemExit("presence.json: o Discord aceita no maximo 2 botoes")
    for button in buttons:
        if not isinstance(button, dict):
            raise SystemExit(f"presence.json: botao precisa ser um objeto: {button}")
        if not str(button.get("url", "")).startswith(("http://", "https://")):
            raise SystemExit(f"presence.json: botao sem URL http(s): {button}")
        if len(button.get("label", "")) > 32:
            raise SystemExit(f"presence.json: label de botao acima de 32 chars: {button}")

    return config


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
            sock.connect(path)
            return sock, path
        except OSError:
            continue
    return None, None


def send(sock, opcode, payload):
    data = json.dumps(payload).encode("utf-8")
    sock.sendall(struct.pack("<II", opcode, len(data)) + data)


def recv(sock):
    header = read_exactly(sock, 8)
    if header is None:
        return None, None
    opcode, length = struct.unpack("<II", header)
    body = read_exactly(sock, length) if length else b""
    if body is None:
        return None, None
    return opcode, json.loads(body.decode("utf-8")) if body else {}


def read_exactly(sock, count):
    chunks = b""
    while len(chunks) < count:
        chunk = sock.recv(count - len(chunks))
        if not chunk:
            return None
        chunks += chunk
    return chunks


def build_activity(config, started_at):
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


def run_session(config, started_at):
    """Uma conexao, do handshake ate cair. Retorna quando o socket morre."""
    sock, path = connect()
    if sock is None:
        return False

    try:
        send(sock, OP_HANDSHAKE, {"v": 1, "client_id": str(config["applicationId"])})
        opcode, payload = recv(sock)
        if opcode is None:
            return False
        if payload.get("evt") == "ERROR":
            log(f"handshake recusado: {payload.get('data')}")
            return False

        user = (payload.get("data") or {}).get("user") or {}
        log(f"conectado em {path} como {user.get('username', '?')}")

        send(sock, OP_FRAME, {
            "cmd": "SET_ACTIVITY",
            "args": {"pid": os.getpid(), "activity": build_activity(config, started_at)},
            "nonce": str(uuid.uuid4()),
        })

        opcode, payload = recv(sock)
        if opcode is None:
            return False
        if payload.get("evt") == "ERROR":
            log(f"SET_ACTIVITY recusado: {payload.get('data')}")
            return False
        log("presenca definida")

        # A partir daqui so escutamos. Nao ha nada a reenviar: a presenca fica
        # de pe enquanto o socket estiver aberto, e some sozinha quando ele
        # fecha. Ler ate o EOF e o jeito de perceber que o Discord fechou.
        while True:
            opcode, payload = recv(sock)
            if opcode is None:
                log("socket encerrado pelo Discord")
                return True
            if opcode == OP_PING:
                send(sock, OP_PONG, payload)
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


def main():
    config = load_config()
    started_at = time.time()

    running = {"value": True}

    def stop(signum, _frame):
        # Fechar o processo ja limpa a presenca: o Discord a descarta quando o
        # socket cai. Nao ha o que "desfazer" no servidor.
        running["value"] = False
        log(f"sinal {signum} recebido, encerrando")
        sys.exit(0)

    signal.signal(signal.SIGTERM, stop)
    signal.signal(signal.SIGINT, stop)

    delay = RECONNECT_MIN
    while running["value"]:
        connected = run_session(config, started_at)
        if connected:
            delay = RECONNECT_MIN
        else:
            delay = min(delay * 2, RECONNECT_MAX)
            log(f"Discord indisponivel, nova tentativa em {delay}s")
        time.sleep(delay)


if __name__ == "__main__":
    main()
