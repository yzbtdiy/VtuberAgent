use std::io::Read;

use crate::errors::Result;
use flate2::read::ZlibDecoder;

pub const HEADER_LEN: u16 = 16;

pub const OP_HEARTBEAT: u32 = 2;
pub const OP_HEARTBEAT_REPLY: u32 = 3;
pub const OP_SEND_EVENT: u32 = 5;
pub const OP_AUTH: u32 = 7;
pub const OP_AUTH_REPLY: u32 = 8;

#[derive(Debug, Clone)]
pub struct BiliPacket {
    pub packet_len: u32,
    pub header_len: u16,
    pub version: u16,
    pub operation: u32,
    pub sequence: u32,
    pub body: Vec<u8>,
}

pub fn encode_packet(operation: u32, body: &[u8]) -> Vec<u8> {
    let header_len = HEADER_LEN;
    let packet_len = header_len as usize + body.len();
    let mut buffer = Vec::with_capacity(packet_len);

    buffer.extend_from_slice(&(packet_len as u32).to_be_bytes());
    buffer.extend_from_slice(&header_len.to_be_bytes());
    buffer.extend_from_slice(&1u16.to_be_bytes()); // version = 1 (plain JSON)
    buffer.extend_from_slice(&operation.to_be_bytes());
    buffer.extend_from_slice(&1u32.to_be_bytes()); // sequence id
    buffer.extend_from_slice(body);

    buffer
}

pub fn decode_packets(data: &[u8]) -> Result<Vec<BiliPacket>> {
    let mut packets = Vec::new();
    let mut offset = 0;

    while offset + HEADER_LEN as usize <= data.len() {
        let packet_len = u32::from_be_bytes(
            data[offset..offset + 4]
                .try_into()
                .expect("slice length checked"),
        ) as usize;

        if packet_len == 0 || offset + packet_len > data.len() {
            break;
        }

        let header_len = u16::from_be_bytes(
            data[offset + 4..offset + 6]
                .try_into()
                .expect("slice length checked"),
        );
        let version = u16::from_be_bytes(
            data[offset + 6..offset + 8]
                .try_into()
                .expect("slice length checked"),
        );
        let operation = u32::from_be_bytes(
            data[offset + 8..offset + 12]
                .try_into()
                .expect("slice length checked"),
        );
        let sequence = u32::from_be_bytes(
            data[offset + 12..offset + 16]
                .try_into()
                .expect("slice length checked"),
        );

        let body_start = offset + header_len as usize;
        let body_end = offset + packet_len;
        let body = data[body_start..body_end].to_vec();

        if version == 2 {
            let mut decoder = ZlibDecoder::new(&body[..]);
            let mut decoded = Vec::new();
            decoder.read_to_end(&mut decoded)?;

            let inner_packets = decode_packets(&decoded)?;
            packets.extend(inner_packets);
        } else {
            packets.push(BiliPacket {
                packet_len: packet_len as u32,
                header_len,
                version,
                operation,
                sequence,
                body,
            });
        }

        offset += packet_len;
    }

    Ok(packets)
}
