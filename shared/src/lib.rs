#![cfg_attr(not(feature = "std"), no_std)]

pub const MAX_CHUNK_SIZE: usize = 1400;

#[derive(Debug)]
pub struct Chunk {
    pub frame_id: u16,
    pub chunk_id: u8,
    pub total_chunks: u8,
    pub payload: [u8; MAX_CHUNK_SIZE],
    pub payload_len: u16,   // how many bytes in payload are actually used
}
// impl ChunkHeader {
//     pub fn to_bytes(&self) -> [u8; 6] {
//         // serialize to bytes — works on both sides
//     }
    
//     pub fn from_bytes(bytes: &[u8]) -> Option<Self> {
//         // deserialize — works on both sides
//     }
// }