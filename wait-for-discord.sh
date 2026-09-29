#!/usr/bin/env bash
#
# Espera o Discord responder por HTTPS antes de o bot subir. Chamado pelo
# ExecStartPre da unit (community-bot.service).
#
# Por que HTTPS e nao so DNS: em 28/09/2026, depois de um reinicio abrupto da
# maquina, o DNS ja resolvia discord.com mas a rede ainda nao estava pronta de
# verdade - por quase um minuto o HTTPS devolveu um certificado que nao era do
# Discord (RemoteCertificateNameMismatch), tipico de roteador ainda subindo. O
# gate antigo, so de DNS, deixou o bot subir nesse intervalo: tres tentativas
# de conexao falharam e o /logs ficou com 30 linhas de erro antes de o bot se
# recuperar sozinho.
#
# O teste de DNS antigo continua aqui como FALLBACK: se o HTTPS nao firmar no
# prazo (curl ausente, Discord fora do ar), ainda vale esperar ao menos o DNS
# resolver, como antes.
#
# Nunca sai com erro: a unit chama com "-" de qualquer jeito, e dali em diante
# quem insiste e o retry do DisCatSharp. O script so decide QUANDO subir.
#
# Os prazos somados (HTTPS + ultimo curl + DNS) ficam abaixo do `timeout` da
# unit, que por sua vez fica abaixo do TimeoutStartSec. Na ordem inversa o
# systemd mata a espera no meio e marca o start como falho - foi o que
# aconteceu com o gate antigo, de 60s, contra os 45s padrao do systemd de
# usuario.

HTTPS_DEADLINE=${HTTPS_DEADLINE:-60}
DNS_DEADLINE=${DNS_DEADLINE:-15}
URL=https://discord.com/api/v10/gateway

# Endpoint publico: responde {"url": "wss://gateway.discord.gg"} sem token. O
# -f faz 4xx/5xx contarem como falha, e a verificacao de certificado do curl e
# justamente o que pega o NameMismatch. -4 porque o IPv6 de saida desta
# maquina nao funciona (mesmo motivo do DOTNET_SYSTEM_NET_DISABLEIPV6).
if command -v curl >/dev/null 2>&1; then
  while (( SECONDS < HTTPS_DEADLINE )); do
    if curl -4 -fsS --max-time 5 -o /dev/null "$URL" 2>/dev/null; then
      echo "[gate] discord.com respondeu por HTTPS apos ${SECONDS}s"
      exit 0
    fi
    sleep 2
  done
  echo "[gate] HTTPS do discord.com nao respondeu em ${HTTPS_DEADLINE}s; caindo para o teste de DNS"
else
  echo "[gate] curl ausente; usando so o teste de DNS"
fi

deadline=$(( SECONDS + DNS_DEADLINE ))
until getent ahostsv4 discord.com >/dev/null 2>&1; do
  if (( SECONDS >= deadline )); then
    echo "[gate] DNS ainda nao resolve discord.com; subindo assim mesmo"
    exit 0
  fi
  sleep 1
done

echo "[gate] DNS resolve discord.com; subindo sem confirmar o HTTPS"
exit 0
