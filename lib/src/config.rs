//! Config: options de/serialized to files
//! from `settings`

use {serde_yaml, PassphraseFn, SGData};

use asyncio;
use chunking;
use compression;

use encryption;
use encryption::{ArcDecrypter, ArcEncrypter};
use hashing;

use hex::ToHex;
use settings;

use std::io;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;

pub const REPO_VERSION_LOWEST: u32 = 0;
pub const REPO_VERSION_CURRENT: u32 = 1;

pub const DATA_SUBDIR: &'static str = "chunk";
pub const NAME_SUBDIR: &'static str = "name";
pub const INDEX_SUBDIR: &'static str = "index";

pub const LOCK_FILE: &'static str = ".lock";
pub const VERSION_FILE: &'static str = "version";
pub const CONFIG_YML_FILE: &'static str = "config.yml";

pub const DEFAULT_BUP_CHUNK_BITS: u32 = 17;

pub fn lock_file_path(path: &Path) -> PathBuf {
    path.join(LOCK_FILE)
}


pub fn write_version_file(
    aio: &asyncio::AsyncIO,
    version: u32,
) -> super::Result<()> {
    let mut v = vec![];
    {
        write!(&mut v, "{}", version)?;
    }

    aio.write(VERSION_FILE.into(), SGData::from_single(v))
        .wait()?;
    Ok(())
}

#[derive(Copy, Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type")]
/// `Chunking` are the algorithms supported by rdedup
pub enum Chunking {
    /// `Bup` is the default algorithm, the chunk_bits value provided with
    /// bup
    /// is the bit shift to be used by rollsum. The valid range is between
    /// 10
    /// and 30 (1KB to 1GB)
    #[serde(rename = "bup")]
    Bup { chunk_bits: u32 },
    #[serde(rename = "gear")]
    Gear { chunk_bits: u32 },
}

/// Default implementation for the `Chunking`
impl Default for Chunking {
    fn default() -> Chunking {
        Chunking::Bup {
            chunk_bits: DEFAULT_BUP_CHUNK_BITS,
        }
    }
}

impl Chunking {
    pub fn valid(self) -> bool {
        match self {
            Chunking::Bup { chunk_bits: bits } => 30 >= bits && bits >= 10,
            Chunking::Gear { chunk_bits: bits } => 30 >= bits && bits >= 10,
        }
    }

    pub(crate) fn to_engine(&self) -> Box<chunking::Chunking> {
        match *self {
            Chunking::Bup { chunk_bits } => Box::new(
                chunking::Bup::new(chunk_bits),
            ),
            Chunking::Gear { chunk_bits } => Box::new(
                chunking::Gear::new(chunk_bits),
            ),
        }
    }
}

#[derive(Copy, Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type")]
pub enum Compression {
    #[serde(rename = "deflate")]
    Deflate,
    #[serde(rename = "xz2")]
    Xz2,
    #[serde(rename = "bzip2")]
    Bzip2,
    #[serde(rename = "zstd")]
    Zstd,
    #[serde(rename = "none")]
    None,
}

impl Default for Compression {
    fn default() -> Compression {
        Compression::Deflate
    }
}

impl Compression {
    pub(crate) fn to_engine(&self) -> compression::ArcCompression {
        match *self {
            Compression::None => Arc::new(compression::NoCompression),
            Compression::Deflate => Arc::new(compression::Deflate),
            Compression::Xz2 => Arc::new(compression::Xz2),
            Compression::Bzip2 => Arc::new(compression::Bzip2),
            Compression::Zstd => Arc::new(compression::Zstd),
        }
    }
}

#[derive(Copy, Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type")]
pub enum Hashing {
    #[serde(rename = "sha256")]
    Sha256,
    #[serde(rename = "blake2b")]
    Blake2b,
}

impl Default for Hashing {
    fn default() -> Hashing {
        Hashing::Sha256
    }
}

impl Hashing {
    pub(crate) fn to_hasher(&self) -> hashing::ArcHasher {
        match *self {
            Hashing::Sha256 => Arc::new(hashing::Sha256),
            Hashing::Blake2b => Arc::new(hashing::Blake2b),
        }
    }
}

/// Types of supported encryption
#[derive(Serialize, Deserialize, Clone)]
#[serde(tag = "type")]
pub enum Encryption {
    /// No encryption
    #[serde(rename = "none")]
    None,
    /// `Curve25519Blake2BSalsa20Poly1305`
    #[serde(rename = "curve25519_blake2b_salsa20_poly1305")]
    Curve25519(encryption::Curve25519),
}

impl encryption::EncryptionEngine for Encryption {
    fn change_passphrase(
        &mut self,
        old_p: PassphraseFn,
        new_p: PassphraseFn,
    ) -> io::Result<()> {
        match *self {
            Encryption::None => Ok(()),
            Encryption::Curve25519(ref mut c) => {
                c.change_passphrase(old_p, new_p)
            }
        }

    }

    fn encrypter(&self, pass: PassphraseFn) -> io::Result<ArcEncrypter> {

        match *self {
            Encryption::None => Ok(Arc::new(encryption::NopEncrypter)),
            Encryption::Curve25519(ref c) => c.encrypter(pass),
        }

    }
    fn decrypter(&self, pass: PassphraseFn) -> io::Result<ArcDecrypter> {

        match *self {
            Encryption::None => Ok(Arc::new(encryption::NopDecrypter)),
            Encryption::Curve25519(ref c) => c.decrypter(pass),
        }
    }
}

#[derive(Deserialize, Serialize, Clone, Debug, PartialEq)]
pub struct Nesting(pub u8);
impl Default for Nesting {
    fn default() -> Self {
        Nesting(2)
    }
}

impl Nesting {
    pub fn get_path(&self, base: &Path, digest: &[u8]) -> PathBuf {
        let hex_digest = &digest.to_hex();
        let mut dir = base.to_path_buf();
        let levels = self.clone().0;
        if levels > 0 {
            for i in 0..levels {
                let start = i as usize * 2;
                let end = start + 2;
                dir = dir.join(&hex_digest[start..end]);
            }
        }
        dir.join(&hex_digest)
    }
}

/// Rdedup repository configuration
///
/// This datastructure is used for serialization and deserialization
/// of repo configuration that is stored as a repostiory metadata.
#[derive(Serialize, Deserialize, Clone)]
pub struct Repo {
    pub version: u32,
    #[serde(default)]
    pub chunking: Chunking,
    pub encryption: Encryption,
    #[serde(default)]
    pub compression: Compression,
    #[serde(default)]
    pub nesting: Nesting,
    #[serde(default)]
    pub hashing: Hashing,
}


impl Repo {
    pub fn new_from_settings(
        pass: PassphraseFn,
        settings: settings::Repo,
    ) -> io::Result<Self> {

        let encryption = match settings.encryption {
            settings::Encryption::Curve25519 => {
                Encryption::Curve25519(encryption::Curve25519::new(pass)?)
            }
            settings::Encryption::None => Encryption::None,
        };

        Ok(Repo {
            version: REPO_VERSION_CURRENT,
            chunking: settings.chunking.0,
            encryption: encryption,
            compression: settings.compression.to_config(),
            nesting: settings.nesting.to_config(),
            hashing: settings.hashing.to_config(),
        })

    }


    pub fn write(&self, aio: &asyncio::AsyncIO) -> super::Result<()> {

        let config_str =
            serde_yaml::to_string(self).expect("yaml serialization failed");

        aio.write(
            CONFIG_YML_FILE.into(),
            SGData::from_single(config_str.into_bytes()),
        ).wait()?;

        write_version_file(aio, REPO_VERSION_CURRENT)?;

        Ok(())
    }
}
