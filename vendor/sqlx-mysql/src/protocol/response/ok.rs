use crate::error::Error;
use crate::io::ProtocolDecode;
use crate::protocol::{response::Status, Capabilities};
use bytes::{Buf, Bytes};

#[derive(Clone)]
pub(crate) enum SessionChange {
    Schema(String),
    SystemVariable { name: String, value: String },
}
impl std::fmt::Debug for SessionChange {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Schema(name) => f.debug_tuple("Schema").field(name).finish(),
            Self::SystemVariable { name, .. } => f
                .debug_struct("SystemVariable")
                .field("name", name)
                .finish_non_exhaustive(),
        }
    }
}

/// Indicates successful completion of a previous command sent by the client.
#[derive(Debug)]
pub struct OkPacket {
    pub affected_rows: u64,
    pub last_insert_id: u64,
    pub status: Status,
    pub warnings: u16,
    pub info: String,
    pub(crate) session_changes: Vec<SessionChange>,
}

// Bound-check every new length-encoded field. Malformed optional state must
// produce a protocol error rather than a Buf underflow or guessed database.
fn lenenc(buf: &mut Bytes) -> Result<u64, Error> {
    if !buf.has_remaining() {
        return Err(err_protocol!("missing length-encoded integer"));
    }
    let prefix = buf.get_u8();
    let count = match prefix {
        0xfc => 2,
        0xfd => 3,
        0xfe => 8,
        0xfb | 0xff => return Err(err_protocol!("invalid length-encoded integer prefix")),
        value => return Ok(u64::from(value)),
    };
    if buf.remaining() < count {
        return Err(err_protocol!("truncated length-encoded integer"));
    }
    Ok(buf.get_uint_le(count))
}
fn string_bytes(buf: &mut Bytes) -> Result<Bytes, Error> {
    let count = usize::try_from(lenenc(buf)?)
        .map_err(|_| err_protocol!("session state length overflows usize"))?;
    if buf.remaining() < count {
        return Err(err_protocol!("truncated session state string"));
    }
    Ok(buf.split_to(count))
}
fn text(buf: &mut Bytes) -> Result<String, Error> {
    String::from_utf8(string_bytes(buf)?.to_vec())
        .map_err(|_| err_protocol!("session state is not valid UTF-8"))
}
impl OkPacket {
    pub(crate) fn decode_capabilities(
        mut buf: Bytes,
        capabilities: Capabilities,
    ) -> Result<Self, Error> {
        if !buf.has_remaining() {
            return Err(err_protocol!("missing OK packet header"));
        }
        let header = buf.get_u8();
        if header != 0 && header != 0xfe {
            return Err(err_protocol!("expected OK packet, found 0x{:02x}", header));
        }
        let affected_rows = lenenc(&mut buf)?;
        let last_insert_id = lenenc(&mut buf)?;
        if buf.remaining() < 4 {
            return Err(err_protocol!("truncated OK packet status"));
        }
        let status = Status::from_bits_truncate(buf.get_u16_le());
        let warnings = buf.get_u16_le();
        let mut session_changes = Vec::new();
        let info = if capabilities.contains(Capabilities::SESSION_TRACK) {
            let info = if buf.has_remaining() {
                String::from_utf8_lossy(&string_bytes(&mut buf)?).into_owned()
            } else {
                String::new()
            };
            if status.contains(Status::SERVER_SESSION_STATE_CHANGED) {
                let mut states = string_bytes(&mut buf)?;
                while states.has_remaining() {
                    let kind = states.get_u8();
                    let mut data = string_bytes(&mut states)?;
                    match kind {
                        0 => {
                            while data.has_remaining() {
                                let name = text(&mut data)?.to_ascii_lowercase();
                                let value_bytes = string_bytes(&mut data)?;
                                // Preserve only observer controls. Other tracked
                                // values are not needed and may contain secrets.
                                if matches!(
                                    name.as_str(),
                                    "session_track_schema" | "session_track_system_variables"
                                ) {
                                    let value =
                                        String::from_utf8(value_bytes.to_vec()).map_err(|_| {
                                            err_protocol!("invalid observer control value")
                                        })?;
                                    session_changes
                                        .push(SessionChange::SystemVariable { name, value });
                                }
                            }
                        }
                        1 => {
                            session_changes.push(SessionChange::Schema(text(&mut data)?));
                            if data.has_remaining() {
                                return Err(err_protocol!(
                                    "unexpected data after schema session state"
                                ));
                            }
                        }
                        _ => {} // Other trackers do not describe database identity.
                    }
                }
            }
            if buf.has_remaining() {
                return Err(err_protocol!("unexpected data after OK session state"));
            }
            info
        } else {
            String::from_utf8_lossy(&buf).into_owned()
        };
        Ok(Self {
            affected_rows,
            last_insert_id,
            status,
            warnings,
            info,
            session_changes,
        })
    }
}
impl ProtocolDecode<'_> for OkPacket {
    fn decode_with(buf: Bytes, _: ()) -> Result<Self, Error> {
        Self::decode_capabilities(buf, Capabilities::empty())
    }
}

#[test]
fn test_decode_ok_packet() {
    const DATA: &[u8] = b"\x00\x00\x00\x02@\x00\x00";

    let p = OkPacket::decode(DATA.into()).unwrap();

    assert_eq!(p.affected_rows, 0);
    assert_eq!(p.last_insert_id, 0);
    assert_eq!(p.warnings, 0);
    assert!(p.status.contains(Status::SERVER_STATUS_AUTOCOMMIT));
    assert!(p.status.contains(Status::SERVER_SESSION_STATE_CHANGED));
}
