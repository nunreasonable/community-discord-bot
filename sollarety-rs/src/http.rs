//! Cliente HTTP compartilhado (HttpClientProvider.cs). Um so por processo,
//! dentro do `Data`: um cliente por chamada acumula sockets em TIME_WAIT.
//!
//! Quem usa: a API publica do Roblox, o Worker de verificacao, Spotify,
//! nekos.best e a busca da musica. A IA local fala com 127.0.0.1 e por isso
//! usa o `local_client`, sem a trava de endereco interno.

use std::{
    net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr},
    sync::Arc,
    time::Duration,
};

use reqwest::{
    Url,
    dns::{Addrs, Name, Resolve, Resolving},
    redirect,
};

pub const USER_AGENT: &str = "Sollarety/1.0 (+https://ccore.daeese.me/fun/)";

/// Quanto cada endereco tem para o handshake TCP. Curto: e so o connect.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
const TIMEOUT: Duration = Duration::from_secs(20);

/// O cliente de tudo que e servico publico. Prazo maior por requisicao vai em
/// `RequestBuilder::timeout`; o padrao e 20 segundos.
pub fn client() -> Result<reqwest::Client, reqwest::Error> {
    reqwest::Client::builder()
        .user_agent(USER_AGENT)
        .dns_resolver(Arc::new(SafeResolver))
        .redirect(redirect::Policy::custom(|attempt| {
            if attempt.previous().len() >= 10 {
                attempt.error("redirecionamentos demais")
            } else if is_blocked_url(attempt.url()) {
                attempt.error("redirecionamento para endereco interno")
            } else {
                attempt.follow()
            }
        }))
        .connect_timeout(CONNECT_TIMEOUT)
        .timeout(TIMEOUT)
        // As conexoes sao recicladas para nao ficar preso a um DNS antigo.
        .pool_idle_timeout(Duration::from_secs(120))
        .pool_max_idle_per_host(20)
        .build()
}

/// Cliente SEM a trava de endereco interno, para a IA local (llama.cpp em
/// 127.0.0.1). Nunca use com URL que venha de usuario.
pub fn local_client(timeout: Duration) -> Result<reqwest::Client, reqwest::Error> {
    reqwest::Client::builder()
        .user_agent(USER_AGENT)
        .connect_timeout(CONNECT_TIMEOUT)
        .timeout(timeout)
        .build()
}

/// A URL e permitida pelo cliente compartilhado? Para conferir ANTES de
/// mandar segredo num header: um IP literal nao passa pelo resolver.
pub fn is_allowed_url(url: &Url) -> bool {
    matches!(url.scheme(), "http" | "https") && !is_blocked_url(url)
}

/// A rede das maquinas do bot anuncia IPv6 mas nao o roteia: o handshake
/// trava. IPv4 vem primeiro. E nada de endereco interno, para uma URL de
/// config adulterada nao levar o bot (e o segredo do header) a rede interna.
struct SafeResolver;

impl Resolve for SafeResolver {
    fn resolve(&self, name: Name) -> Resolving {
        Box::pin(async move {
            let found = tokio::net::lookup_host((name.as_str(), 0)).await?;
            let mut allowed: Vec<SocketAddr> =
                found.filter(|addr| !is_blocked(addr.ip())).collect();

            if allowed.is_empty() {
                return Err(format!("{} nao tem endereco permitido", name.as_str()).into());
            }

            allowed.sort_by_key(SocketAddr::is_ipv6);
            let addrs: Addrs = Box::new(allowed.into_iter());
            Ok(addrs)
        })
    }
}

/// O resolver nao ve URL com IP literal, entao esse caso e conferido aqui.
fn is_blocked_url(url: &Url) -> bool {
    match url.host() {
        Some(url::Host::Ipv4(ip)) => is_blocked(IpAddr::V4(ip)),
        Some(url::Host::Ipv6(ip)) => is_blocked(IpAddr::V6(ip)),
        Some(url::Host::Domain(host)) => host.eq_ignore_ascii_case("localhost"),
        None => true,
    }
}

/// Loopback, "qualquer", privados (RFC 1918), CGNAT, link-local (inclui o
/// 169.254.169.254 de metadata) e o equivalente IPv6.
pub fn is_blocked(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(ip) => is_blocked_v4(ip),
        IpAddr::V6(ip) => is_blocked_v6(ip),
    }
}

fn is_blocked_v4(ip: Ipv4Addr) -> bool {
    let [a, b, ..] = ip.octets();
    let cgnat = a == 100 && (64..=127).contains(&b);

    ip.is_loopback()
        || ip.is_private()
        || ip.is_link_local()
        || ip.is_unspecified()
        || ip.is_broadcast()
        || cgnat
}

fn is_blocked_v6(ip: Ipv6Addr) -> bool {
    if let Some(mapped) = ip.to_ipv4_mapped() {
        return is_blocked_v4(mapped);
    }

    let site_local = (ip.segments()[0] & 0xffc0) == 0xfec0;

    ip.is_loopback()
        || ip.is_unspecified()
        || ip.is_unique_local()
        || ip.is_unicast_link_local()
        || site_local
}

#[cfg(test)]
mod tests {
    use super::*;

    fn blocked(text: &str) -> bool {
        is_blocked(text.parse().unwrap())
    }

    #[test]
    fn recusa_enderecos_internos() {
        for ip in [
            "127.0.0.1",
            "10.1.2.3",
            "172.16.0.1",
            "192.168.0.1",
            "169.254.169.254",
            "100.64.0.1",
            "0.0.0.0",
            "255.255.255.255",
            "::1",
            "::",
            "fc00::1",
            "fe80::1",
            "fec0::1",
            "::ffff:192.168.0.1",
        ] {
            assert!(blocked(ip), "{ip} deveria ser recusado");
        }
    }

    #[test]
    fn aceita_enderecos_publicos() {
        for ip in ["8.8.8.8", "172.32.0.1", "2606:4700::1111"] {
            assert!(!blocked(ip), "{ip} deveria passar");
        }
    }

    #[test]
    fn recusa_url_interna() {
        assert!(!is_allowed_url(
            &Url::parse("http://169.254.169.254/x").unwrap()
        ));
        assert!(!is_allowed_url(
            &Url::parse("http://localhost:9931/").unwrap()
        ));
        assert!(!is_allowed_url(&Url::parse("file:///etc/passwd").unwrap()));
        assert!(is_allowed_url(
            &Url::parse("https://daeese.me/oauth/roblox/result").unwrap()
        ));
    }
}
