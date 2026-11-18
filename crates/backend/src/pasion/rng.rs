//! Bridge the current ChaCha generator to Pasion's rand_core 0.6 API.
use anyhow::Context;
use rand_chacha::{
    ChaCha20Rng,
    rand_core::{RngCore, SeedableRng},
};

pub(super) struct PasionRng(ChaCha20Rng);

impl PasionRng {
    pub(super) fn from_os_rng() -> anyhow::Result<Self> {
        ChaCha20Rng::try_from_os_rng()
            .map(Self)
            .context("could not seed Pasion RNG from operating system entropy")
    }
}

impl rand_core_06::RngCore for PasionRng {
    fn next_u32(&mut self) -> u32 {
        self.0.next_u32()
    }

    fn next_u64(&mut self) -> u64 {
        self.0.next_u64()
    }

    fn fill_bytes(&mut self, dest: &mut [u8]) {
        self.0.fill_bytes(dest);
    }

    fn try_fill_bytes(&mut self, dest: &mut [u8]) -> Result<(), rand_core_06::Error> {
        self.0.fill_bytes(dest);
        Ok(())
    }
}

// ChaCha20Rng is a CryptoRng; the bridge forwards its output unchanged.
impl rand_core_06::CryptoRng for PasionRng {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pasion_rng_preserves_chacha20_output_and_crypto_bound() {
        fn requires_crypto_rng(_: &mut (impl rand_core_06::RngCore + rand_core_06::CryptoRng)) {}
        let mut expected = ChaCha20Rng::from_seed([42; 32]);
        let mut actual = PasionRng(expected.clone());
        requires_crypto_rng(&mut actual);
        assert_eq!(
            rand_core_06::RngCore::next_u32(&mut actual),
            expected.next_u32()
        );
        assert_eq!(
            rand_core_06::RngCore::next_u64(&mut actual),
            expected.next_u64()
        );
        for size in [0, 1, 31, 64, 65, 257] {
            let mut expected_bytes = vec![0; size];
            let mut actual_bytes = vec![0; size];
            expected.fill_bytes(&mut expected_bytes);
            rand_core_06::RngCore::fill_bytes(&mut actual, &mut actual_bytes);
            assert_eq!(actual_bytes, expected_bytes);
            expected.fill_bytes(&mut expected_bytes);
            rand_core_06::RngCore::try_fill_bytes(&mut actual, &mut actual_bytes).unwrap();
            assert_eq!(actual_bytes, expected_bytes);
        }
    }
}
