use crate::model::{Credentials, Profile};
use aes_gcm::{
    Aes256Gcm, Key, Nonce,
    aead::{Aead, Generate, KeyInit, Payload},
};
use anyhow::{Context, Result};
use directories::ProjectDirs;
use serde::{Deserialize, Serialize};
use std::{collections::HashMap, fs, io::Write, path::PathBuf};
use uuid::Uuid;

const CREDENTIAL_HEADER: &[u8] = b"SQLSHOP-CREDENTIALS-V1\0";
const NONCE_LENGTH: usize = 12;

// Serialized only inside the encrypted credential file; never in Profile or logs.
#[derive(Serialize, Deserialize)]
struct SavedCredentials {
    database_password: String,
    ssh_secret: String,
}

pub struct Store {
    pub directory: PathBuf,
}

impl Store {
    pub fn local() -> Result<Self> {
        let dirs =
            ProjectDirs::from("dev", "sqlshop", "SQLShop").context("无法定位本机配置目录")?;
        Ok(Self {
            directory: dirs.config_dir().into(),
        })
    }

    pub fn load(&self) -> Result<Vec<Profile>> {
        let data = match fs::read(self.directory.join("connections.json")) {
            Ok(data) => data,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => return Err(error).context("读取连接配置失败"),
        };
        serde_json::from_slice(&data).context("连接配置损坏，请检查 connections.json")
    }

    pub fn save(&self, profiles: &[Profile]) -> Result<()> {
        self.atomic_write("connections.json", &serde_json::to_vec_pretty(profiles)?)
            .context("保存连接配置失败")
    }

    pub fn credentials(&self, profile: &Profile) -> Result<Option<Credentials>> {
        Ok(self
            .read_credentials()?
            .remove(&profile.id)
            .map(|saved| Credentials {
                database_password: if profile.remember_password {
                    saved.database_password
                } else {
                    String::new()
                },
                ssh_secret: if profile.ssh.enabled && profile.ssh.remember_secret {
                    saved.ssh_secret
                } else {
                    String::new()
                },
            }))
    }

    pub fn save_credentials(&self, profile: &Profile, credentials: &Credentials) -> Result<()> {
        let mut saved = self.read_credentials()?;
        if profile.remember_password || (profile.ssh.enabled && profile.ssh.remember_secret) {
            saved.insert(
                profile.id,
                SavedCredentials {
                    database_password: if profile.remember_password {
                        credentials.database_password.clone()
                    } else {
                        String::new()
                    },
                    ssh_secret: if profile.ssh.enabled && profile.ssh.remember_secret {
                        credentials.ssh_secret.clone()
                    } else {
                        String::new()
                    },
                },
            );
        } else if saved.remove(&profile.id).is_none() {
            return Ok(());
        }
        self.write_credentials(&saved)
    }

    pub fn forget_credentials(&self, id: Uuid) -> Result<()> {
        let mut saved = self.read_credentials()?;
        if saved.remove(&id).is_some() {
            self.write_credentials(&saved)?;
        }
        Ok(())
    }

    fn read_credentials(&self) -> Result<HashMap<Uuid, SavedCredentials>> {
        let data = match fs::read(self.directory.join("credentials.enc")) {
            Ok(data) => data,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(HashMap::new()),
            Err(error) => return Err(error).context("读取本机凭据失败"),
        };
        anyhow::ensure!(
            data.starts_with(CREDENTIAL_HEADER)
                && data.len() >= CREDENTIAL_HEADER.len() + NONCE_LENGTH + 16,
            "本机凭据文件损坏，未修改原文件"
        );
        let key = self.credential_key(false)?;
        let cipher = Aes256Gcm::new(&key);
        let start = CREDENTIAL_HEADER.len();
        let nonce_bytes: [u8; NONCE_LENGTH] = data[start..start + NONCE_LENGTH].try_into()?;
        let nonce = Nonce::from(nonce_bytes);
        let plaintext = cipher
            .decrypt(
                &nonce,
                Payload {
                    msg: &data[start + NONCE_LENGTH..],
                    aad: CREDENTIAL_HEADER,
                },
            )
            .map_err(|_| {
                anyhow::anyhow!(
                    "本机凭据解密失败，请检查 credentials.enc 和 credentials.key，原文件未修改"
                )
            })?;
        serde_json::from_slice(&plaintext).context("本机凭据内容损坏，原文件未修改")
    }

    fn write_credentials(&self, saved: &HashMap<Uuid, SavedCredentials>) -> Result<()> {
        if saved.is_empty() {
            match fs::remove_file(self.directory.join("credentials.enc")) {
                Ok(()) => return Ok(()),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
                Err(error) => return Err(error).context("删除本机凭据失败"),
            }
        }
        let key = self.credential_key(true)?;
        let cipher = Aes256Gcm::new(&key);
        let nonce = Nonce::try_generate()
            .map_err(|error| anyhow::anyhow!("生成凭据随机数失败：{error}"))?;
        let plaintext = serde_json::to_vec(saved)?;
        let encrypted = cipher
            .encrypt(
                &nonce,
                Payload {
                    msg: &plaintext,
                    aad: CREDENTIAL_HEADER,
                },
            )
            .map_err(|_| anyhow::anyhow!("加密本机凭据失败"))?;
        let mut data = CREDENTIAL_HEADER.to_vec();
        data.extend_from_slice(&nonce);
        data.extend_from_slice(&encrypted);
        self.atomic_write("credentials.enc", &data)
            .context("保存本机凭据失败")
    }

    fn credential_key(&self, create: bool) -> Result<Key<Aes256Gcm>> {
        let path = self.directory.join("credentials.key");
        match fs::read(&path) {
            Ok(data) => Key::<Aes256Gcm>::try_from(data.as_slice()).context("本机凭据密钥损坏"),
            Err(error) if create && error.kind() == std::io::ErrorKind::NotFound => {
                fs::create_dir_all(&self.directory)?;
                let key = Key::<Aes256Gcm>::try_generate()
                    .map_err(|error| anyhow::anyhow!("生成本机凭据密钥失败：{error}"))?;
                // Never replace an existing key: it would make saved passwords unreadable.
                let mut options = fs::OpenOptions::new();
                options.write(true).create_new(true);
                #[cfg(unix)]
                {
                    use std::os::unix::fs::OpenOptionsExt;
                    options.mode(0o600);
                }
                match options.open(&path) {
                    Ok(mut file) => {
                        file.write_all(&key)?;
                        file.sync_all()?;
                        Ok(key)
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                        self.credential_key(false)
                    }
                    Err(error) => Err(error).context("保存本机凭据密钥失败"),
                }
            }
            Err(error) => Err(error).context("读取本机凭据密钥失败，请检查 credentials.key"),
        }
    }

    fn atomic_write(&self, name: &str, data: &[u8]) -> Result<()> {
        fs::create_dir_all(&self.directory)?;
        let path = self.directory.join(name);
        let temporary = self
            .directory
            .join(format!("{name}.{}.tmp", Uuid::new_v4()));
        let result = (|| -> Result<()> {
            let mut options = fs::OpenOptions::new();
            options.write(true).create_new(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.mode(0o600);
            }
            let mut file = options.open(&temporary)?;
            file.write_all(data)?;
            file.sync_all()?;
            fs::rename(&temporary, &path)?;
            Ok(())
        })();
        if result.is_err() {
            let _ = fs::remove_file(temporary);
        }
        result
    }
}
