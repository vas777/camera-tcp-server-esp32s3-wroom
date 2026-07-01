#![cfg_attr(not(feature = "std"), no_std)]

pub const MAX_CHUNK_SIZE: usize = 1400;
pub const STRUCT_CHUNK_SIZE: usize = MAX_CHUNK_SIZE + 6;

#[derive(Debug, Clone)]
pub struct JpegFrameChunk {
    pub frame_id: u16,
    pub chunk_id: u8,
    pub total_chunks: u8,
    pub payload_len: u16,
    pub payload: [u8; MAX_CHUNK_SIZE],
}

impl JpegFrameChunk {
    #[must_use]
    pub fn to_bytes(&self) -> [u8; STRUCT_CHUNK_SIZE] {
        let mut res: [u8; STRUCT_CHUNK_SIZE] = [0u8; STRUCT_CHUNK_SIZE];
        res[0..2].copy_from_slice(&self.frame_id.to_be_bytes());
        res[2] = self.chunk_id;
        res[3] = self.total_chunks;
        res[4..6].copy_from_slice(&self.payload_len.to_be_bytes());
        res[6..STRUCT_CHUNK_SIZE].copy_from_slice(&self.payload);
        res
    }

    #[must_use]
    pub fn from_bytes(bytes: &[u8]) -> Option<Self> {
        if bytes.len() < STRUCT_CHUNK_SIZE {
            None
        } else {
            Some(Self {
                frame_id: u16::from_be_bytes(bytes[0..2].try_into().ok()?),
                chunk_id: bytes[2],
                total_chunks: bytes[3],
                payload_len: u16::from_be_bytes(bytes[4..6].try_into().ok()?),
                payload: bytes[6..STRUCT_CHUNK_SIZE].try_into().ok()?,
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::{JpegFrameChunk, MAX_CHUNK_SIZE};
    // TODO add more comprehensive  testing
    // use quickcheck::*;
    // use rand::*;

    // #[derive(Debug, Clone)]
    // struct ValidPasswordFixture(pub JpegFrameChunk);

    // impl quickcheck::Arbitrary for ValidPasswordFixture {
    //     fn arbitrary(g: &mut Gen) -> Self {
    //         let seed: u64 = g.size() as u64;
    //         let mut rng = rand::rngs::SmallRng::seed_from_u64(seed);
    //         let password: String = FakePassword(8..30).fake_with_rng(&mut rng);
    //         Self(SecretString::new(password.into_boxed_str()))
    //     }
    // }

    // #[test]
    // fn valid_passwords_are_parsed_successfully() {
    //     fn property(valid_password: ValidPasswordFixture) -> bool {
    //             HashedPassword::parse(valid_password.0).await.is_ok()
    //     }

    //     // 10 tests ~ 2 seconds =)
    //     quickcheck::QuickCheck::new()
    //         .tests(10) // Set your custom number of test runs here
    //         .quickcheck(property as fn(ValidPasswordFixture) -> bool);
    // }

    #[test]
    fn test_jpeg_fram_chunk() {
        // TODO : try quickcheck ?
        let frame1 = JpegFrameChunk {
            frame_id: 255,
            chunk_id: 254,
            total_chunks: 254,
            payload_len: MAX_CHUNK_SIZE as u16,
            payload: [1u8; MAX_CHUNK_SIZE],
        };

        let frame_bytes = frame1.to_bytes();

        let frame_from_bytes = JpegFrameChunk::from_bytes(&frame_bytes).unwrap();

        assert_eq!(frame1.frame_id, frame_from_bytes.frame_id);
        assert_eq!(frame1.chunk_id, frame_from_bytes.chunk_id);
        assert_eq!(frame1.total_chunks, frame_from_bytes.total_chunks);
        assert_eq!(frame1.payload_len, frame_from_bytes.payload_len);
        assert_eq!(frame1.payload, frame_from_bytes.payload);
    }
}
