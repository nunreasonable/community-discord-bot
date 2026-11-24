//! StableHash.cs: o /rate e o /text mock precisam de um hash ESTAVEL entre
//! reinicios do bot, diferente do GetHashCode do .NET, que e aleatorizado por
//! processo.

use sha2::{Digest, Sha256};

/// StableHash.Of do C#: os primeiros 4 bytes do SHA-256 lidos como
/// little-endian (BinaryPrimitives.ReadUInt32LittleEndian).
pub fn stable_hash_of(text: &str) -> u32 {
    let digest = Sha256::digest(text.as_bytes());
    u32::from_le_bytes([digest[0], digest[1], digest[2], digest[3]])
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Vetores tirados rodando o StableHash.Of real (SHA256.HashData +
    /// ReadUInt32LittleEndian) num `dotnet run` fora do repo, para garantir
    /// que o resultado bate com o C#.
    #[test]
    fn bate_com_o_stablehash_do_csharp() {
        assert_eq!(stable_hash_of("hello world") as i32, -1188606535);
        assert_eq!(stable_hash_of("Sollarety is cool") as i32, -771374900);
        assert_eq!(stable_hash_of("a") as i32, 310482890);
        assert_eq!(stable_hash_of("The Quick Brown Fox") as i32, 1127929564);
    }
}
