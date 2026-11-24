//! Um CancellationToken minimo (o tokio-util nao esta no Cargo.toml): um sinal
//! que so vai de "vivo" para "cancelado", com filhos que caem junto com o pai.
//! O player tem um para a vida inteira e um filho por faixa (o skip).

use std::sync::Arc;

use tokio::sync::watch;

#[derive(Clone)]
pub struct Token(Arc<Inner>);

struct Inner {
    flag: watch::Sender<bool>,
    parent: Option<Token>,
}

impl Default for Token {
    fn default() -> Self {
        Self::new()
    }
}

impl Token {
    pub fn new() -> Self {
        Self(Arc::new(Inner {
            flag: watch::Sender::new(false),
            parent: None,
        }))
    }

    /// Cancelado quando este OU o pai for cancelado; cancelar o filho nao
    /// mexe no pai.
    pub fn child(&self) -> Self {
        Self(Arc::new(Inner {
            flag: watch::Sender::new(false),
            parent: Some(self.clone()),
        }))
    }

    pub fn cancel(&self) {
        self.0.flag.send_replace(true);
    }

    pub fn is_cancelled(&self) -> bool {
        *self.0.flag.borrow() || self.0.parent.as_ref().is_some_and(Token::is_cancelled)
    }

    /// Termina quando o token (ou um ancestral) for cancelado.
    pub async fn cancelled(&self) {
        let mut own = self.0.flag.subscribe();
        // O Sender mora no proprio token, entao o wait_for nunca ve o canal
        // fechado enquanto alguem espera aqui.
        let own = async move {
            let _ = own.wait_for(|cancelled| *cancelled).await;
        };
        match &self.0.parent {
            None => own.await,
            Some(parent) => {
                let parent = Box::pin(parent.cancelled());
                tokio::select! {
                    () = own => {}
                    () = parent => {}
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn filho_cai_com_o_pai_e_nao_o_contrario() {
        let parent = Token::new();
        let child = parent.child();
        child.cancel();
        assert!(child.is_cancelled());
        assert!(!parent.is_cancelled());

        let other = parent.child();
        let waiter = tokio::spawn({
            let other = other.clone();
            async move { other.cancelled().await }
        });
        parent.cancel();
        waiter.await.unwrap();
        assert!(other.is_cancelled());
        // Ja cancelado: volta na hora.
        other.cancelled().await;
    }
}
