//! Socket-scoped device proof; the existing seed and short display fingerprint stay stable.
use anyhow::{bail, Context, Result};
use base64::{engine::general_purpose::STANDARD, Engine};
use ed25519_dalek::Signer;
use serde_json::{json, Value};
use std::{path::Path, time::Duration};
use tokio_tungstenite::tungstenite::Message;
use tokio_util::sync::CancellationToken;

pub struct Identity(ed25519_dalek::SigningKey);

impl Identity {
    pub fn load(state: &Path) -> Result<Self> {
        let path = state.join("identity.key");
        let seed = match std::fs::read(&path) {
            Ok(bytes) => bytes
                .try_into()
                .map_err(|_| anyhow::anyhow!("invalid identity.key; existing identity was preserved"))?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                use std::io::Write;
                let mut seed = [0u8; 32];
                getrandom::fill(&mut seed)?;
                let mut options = std::fs::OpenOptions::new();
                options.write(true).create_new(true);
                #[cfg(unix)]
                {
                    use std::os::unix::fs::OpenOptionsExt;
                    options.mode(0o600);
                }
                let mut file = options.open(&path)?;
                file.write_all(&seed)?;
                file.sync_all()?;
                seed
            }
            Err(e) => return Err(e.into()),
        };
        Ok(Self(ed25519_dalek::SigningKey::from_bytes(&seed)))
    }

    pub fn fingerprint(&self) -> String {
        data_encoding::BASE32_NOPAD.encode(&self.0.verifying_key().to_bytes()[..8])
    }

    pub fn signed_join(&self, challenge: &Value, info: Value) -> Result<Value> {
        anyhow::ensure!(
            challenge["k"] == "node-challenge",
            "Hub must provide a device identity challenge; unsigned fallback is disabled"
        );
        let nonce = challenge["node_nonce"].as_str().context("missing device challenge nonce")?;
        anyhow::ensure!(
            nonce.len() == 64 && nonce.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)),
            "invalid device challenge nonce"
        );
        let expiry = challenge["node_expires_at"].as_i64().context("missing device challenge expiry")?;
        let now = chrono::Utc::now().timestamp();
        anyhow::ensure!(
            expiry > now && expiry <= now + 60,
            "device identity challenge expired or outside allowed lifetime"
        );
        let public_key = STANDARD.encode(self.0.verifying_key().to_bytes());
        let fp = self.fingerprint();
        let proof = format!("handwave-node-identity-v1\n{nonce}\n{expiry}\n{fp}\n{public_key}");
        let signature = STANDARD.encode(self.0.sign(proof.as_bytes()).to_bytes());
        Ok(
            json!({"k":"join","fp":fp,"info":info,"node_public_key":public_key,"node_nonce":nonce,"node_expires_at":expiry,"node_signature":signature}),
        )
    }
}

pub async fn authenticate(
    socket: &mut crate::connection::Socket,
    settings: &crate::config::Resolved,
    quit: CancellationToken,
) -> Result<()> {
    let handshake = async {
        let challenge = next_json(socket).await?;
        let join = settings.signed_join(&challenge)?;
        socket.send(Message::Text(join.to_string().into())).await?;
        let ack = next_json(socket).await?;
        anyhow::ensure!(
            ack["k"] == "node-joined" && ack["fp"] == settings.fp,
            "Hub did not acknowledge device identity; unsigned fallback is disabled"
        );
        Ok(())
    };
    tokio::select! {
        _ = quit.cancelled() => bail!("connection shutdown requested during device authentication"),
        result = tokio::time::timeout(Duration::from_secs(35), handshake) => result.context("device identity handshake timed out")?,
    }
}

async fn next_json(socket: &mut crate::connection::Socket) -> Result<Value> {
    loop {
        match socket.next().await.context("Hub closed before device authentication")?? {
            Message::Text(text) => return serde_json::from_str(&text).context("invalid device authentication message"),
            Message::Ping(data) => socket.send(Message::Pong(data)).await?,
            Message::Pong(_) => {}
            _ => bail!("Hub closed or sent an unsupported device authentication message"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::Verifier;

    #[test]
    fn existing_seed_is_preserved_and_proof_has_a_fixed_domain() {
        let dir = std::env::temp_dir().join(format!("handwave-identity-{}", crate::wire::fresh_id()));
        std::fs::create_dir(&dir).unwrap();
        let seed = [7u8; 32];
        std::fs::write(dir.join("identity.key"), seed).unwrap();
        let key = Identity::load(&dir).unwrap();
        let expected = ed25519_dalek::SigningKey::from_bytes(&seed);
        assert_eq!(key.fingerprint(), data_encoding::BASE32_NOPAD.encode(&expected.verifying_key().to_bytes()[..8]));
        let expiry = chrono::Utc::now().timestamp() + 30;
        let nonce = "a".repeat(64);
        let challenge = json!({"k":"node-challenge","node_nonce":nonce,"node_expires_at":expiry});
        let join = key.signed_join(&challenge, json!({"hostname":"fixture"})).unwrap();
        let proof = format!(
            "handwave-node-identity-v1\n{nonce}\n{expiry}\n{}\n{}",
            key.fingerprint(),
            join["node_public_key"].as_str().unwrap()
        );
        let signature =
            ed25519_dalek::Signature::from_slice(&STANDARD.decode(join["node_signature"].as_str().unwrap()).unwrap())
                .unwrap();
        expected.verifying_key().verify(proof.as_bytes(), &signature).unwrap();
        assert!(expected
            .verifying_key()
            .verify(proof.replace("identity-v1", "another-domain").as_bytes(), &signature)
            .is_err());
        assert_eq!(std::fs::read(dir.join("identity.key")).unwrap(), seed);
        std::fs::write(dir.join("identity.key"), b"damaged").unwrap();
        assert!(Identity::load(&dir).is_err());
        assert_eq!(std::fs::read(dir.join("identity.key")).unwrap(), b"damaged");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn malformed_expired_or_old_unsigned_challenges_are_rejected() {
        let identity = Identity(ed25519_dalek::SigningKey::from_bytes(&[9u8; 32]));
        let now = chrono::Utc::now().timestamp();
        for challenge in [
            json!({"k":"paired"}),
            json!({"k":"node-challenge","node_nonce":"a".repeat(64),"node_expires_at":now}),
            json!({"k":"node-challenge","node_nonce":"a".repeat(64),"node_expires_at":now+61}),
            json!({"k":"node-challenge","node_nonce":"not hex","node_expires_at":now+30}),
        ] {
            assert!(identity.signed_join(&challenge, json!({})).is_err());
        }
    }
}
