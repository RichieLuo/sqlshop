use std::collections::VecDeque;
use std::ops::{Deref, DerefMut};

use bytes::{Buf, Bytes, BytesMut};

use crate::collation::{CharSet, Collation};
use crate::error::Error;
use crate::io::MySqlBufExt;
use crate::io::{ProtocolDecode, ProtocolEncode};
use crate::net::{BufferedSocket, Socket};
use crate::protocol::response::{EofPacket, ErrPacket, OkPacket, SessionChange, Status};
use crate::protocol::{Capabilities, Packet};
use crate::{MySqlConnectOptions, MySqlDatabaseError};

pub struct MySqlStream<S = Box<dyn Socket>> {
    // Wrapping the socket in `Box` allows us to unsize in-place.
    pub(crate) socket: BufferedSocket<S>,
    pub(crate) server_version: (u16, u16, u16),
    pub(super) capabilities: Capabilities,
    pub(crate) sequence_id: u8,
    pub(crate) waiting: VecDeque<Waiting>,
    pub(crate) charset: CharSet,
    pub(crate) collation: Collation,
    pub(crate) is_tls: bool,
    pub(crate) connection_id: u32,
    pub(crate) observed_status: Option<Status>,
    pub(crate) current_database: Option<String>,
    pub(crate) database_confirmed: bool,
    pub(crate) schema_tracking_enabled: bool,
    pub(crate) system_tracking_all: bool,
    pub(crate) context_tracking_initialized: bool,
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Waiting {
    // waiting for a result set
    Result,

    // waiting for a row within a result set
    Row,
}

impl<S: Socket> MySqlStream<S> {
    pub(crate) fn observe_completion(&mut self, status: Status, changes: &[SessionChange]) {
        self.observed_status = Some(status);
        for change in changes {
            match change {
                SessionChange::Schema(name) => {
                    self.current_database = (!name.is_empty()).then(|| name.clone());
                    self.database_confirmed = true;
                }
                SessionChange::SystemVariable { name, value } => match name.as_str() {
                    "session_track_schema" => {
                        self.schema_tracking_enabled =
                            matches!(value.to_ascii_uppercase().as_str(), "ON" | "1" | "TRUE");
                        if !self.schema_tracking_enabled {
                            self.database_confirmed = false;
                        }
                    }
                    "session_track_system_variables" => {
                        self.system_tracking_all = value.trim() == "*";
                        if !self.system_tracking_all {
                            self.database_confirmed = false;
                        }
                    }
                    _ => {}
                },
            }
        }
        if status.contains(Status::SERVER_STATUS_DB_DROPPED) {
            self.current_database = None;
            self.database_confirmed = true;
        }
    }

    pub(crate) fn with_socket(
        charset: CharSet,
        collation: Collation,
        options: &MySqlConnectOptions,
        socket: S,
    ) -> Self {
        let mut capabilities = Capabilities::PROTOCOL_41
            | Capabilities::IGNORE_SPACE
            | Capabilities::DEPRECATE_EOF
            | Capabilities::FOUND_ROWS
            | Capabilities::TRANSACTIONS
            | Capabilities::SECURE_CONNECTION
            | Capabilities::PLUGIN_AUTH_LENENC_DATA
            | Capabilities::MULTI_STATEMENTS
            | Capabilities::MULTI_RESULTS
            | Capabilities::PLUGIN_AUTH
            | Capabilities::PS_MULTI_RESULTS
            | Capabilities::SSL
            | Capabilities::SESSION_TRACK;

        if options.database.is_some() {
            capabilities |= Capabilities::CONNECT_WITH_DB;
        }

        Self {
            waiting: VecDeque::new(),
            capabilities,
            server_version: (0, 0, 0),
            sequence_id: 0,
            collation,
            charset,
            socket: BufferedSocket::new(socket),
            is_tls: false,
            connection_id: 0,
            observed_status: None,
            current_database: options.database.clone(),
            database_confirmed: false,
            schema_tracking_enabled: false,
            system_tracking_all: false,
            context_tracking_initialized: false,
        }
    }

    pub(crate) async fn wait_until_ready(&mut self) -> Result<(), Error> {
        if !self.socket.write_buffer().is_empty() {
            self.socket.flush().await?;
        }

        while !self.waiting.is_empty() {
            while self.waiting.front() == Some(&Waiting::Row) {
                let packet = self.recv_packet().await?;

                if packet.is_result_end(self.capabilities) {
                    let eof = packet.eof(self.capabilities)?;
                    self.observe_completion(eof.status, &eof.session_changes);

                    if eof.status.contains(Status::SERVER_MORE_RESULTS_EXISTS) {
                        *self.waiting.front_mut().unwrap() = Waiting::Result;
                    } else {
                        self.waiting.pop_front();
                    };
                }
            }

            while self.waiting.front() == Some(&Waiting::Result) {
                let packet = self.recv_packet().await?;

                if !packet.is_empty() && (packet[0] == 0x00 || packet[0] == 0xff) {
                    let ok = packet.ok_with(self.capabilities)?;
                    self.observe_completion(ok.status, &ok.session_changes);

                    if !ok.status.contains(Status::SERVER_MORE_RESULTS_EXISTS) {
                        self.waiting.pop_front();
                    }
                } else {
                    *self.waiting.front_mut().unwrap() = Waiting::Row;
                    self.skip_result_metadata(packet).await?;
                }
            }
        }

        Ok(())
    }

    pub(crate) async fn send_packet<'en, T>(&mut self, payload: T) -> Result<(), Error>
    where
        T: ProtocolEncode<'en, Capabilities>,
    {
        self.sequence_id = 0;
        self.write_packet(payload)?;
        self.flush().await?;
        Ok(())
    }

    pub(crate) fn write_packet<'en, T>(&mut self, payload: T) -> Result<(), Error>
    where
        T: ProtocolEncode<'en, Capabilities>,
    {
        self.socket
            .write_with(Packet(payload), (self.capabilities, &mut self.sequence_id))
    }

    async fn recv_packet_part(&mut self) -> Result<Bytes, Error> {
        // https://dev.mysql.com/doc/dev/mysql-server/8.0.12/page_protocol_basic_packets.html
        // https://mariadb.com/kb/en/library/0-packet/#standard-packet

        let mut header: Bytes = self.socket.read(4).await?;

        // cannot overflow
        #[allow(clippy::cast_possible_truncation)]
        let packet_size = header.get_uint_le(3) as usize;
        let sequence_id = header.get_u8();

        self.sequence_id = sequence_id.wrapping_add(1);

        let payload: Bytes = self.socket.read(packet_size).await?;

        // TODO: packet compression

        Ok(payload)
    }

    // receive the next packet from the database server
    // may block (async) on more data from the server
    pub(crate) async fn recv_packet(&mut self) -> Result<Packet<Bytes>, Error> {
        let payload = self.recv_packet_part().await?;
        let payload = if payload.len() < 0xFF_FF_FF {
            payload
        } else {
            let mut final_payload = BytesMut::with_capacity(0xFF_FF_FF * 2);
            final_payload.extend_from_slice(&payload);

            drop(payload); // we don't need the allocation anymore

            let mut last_read = 0xFF_FF_FF;
            while last_read == 0xFF_FF_FF {
                let part = self.recv_packet_part().await?;
                last_read = part.len();
                final_payload.extend_from_slice(&part);
            }
            final_payload.into()
        };

        if payload
            .first()
            .ok_or(err_protocol!("Packet empty"))?
            .eq(&0xff)
        {
            self.waiting.pop_front();
            // ERR packets carry no transaction status (e.g. deadlock rollback).
            self.observed_status = None;
            self.database_confirmed = false;

            // instead of letting this packet be looked at everywhere, we check here
            // and emit a proper Error
            return Err(
                MySqlDatabaseError(ErrPacket::decode_with(payload, self.capabilities)?).into(),
            );
        }

        Ok(Packet(payload))
    }

    pub(crate) async fn recv<'de, T>(&mut self) -> Result<T, Error>
    where
        T: ProtocolDecode<'de, Capabilities>,
    {
        self.recv_packet().await?.decode_with(self.capabilities)
    }

    pub(crate) async fn recv_ok(&mut self) -> Result<OkPacket, Error> {
        let ok = self.recv_packet().await?.ok_with(self.capabilities)?;
        self.observe_completion(ok.status, &ok.session_changes);
        Ok(ok)
    }

    pub(crate) async fn maybe_recv_eof(&mut self) -> Result<Option<EofPacket>, Error> {
        if self.capabilities.contains(Capabilities::DEPRECATE_EOF) {
            Ok(None)
        } else {
            self.recv().await.map(Some)
        }
    }

    async fn skip_result_metadata(&mut self, mut packet: Packet<Bytes>) -> Result<(), Error> {
        let num_columns: u64 = packet.get_uint_lenenc(); // column count

        for _ in 0..num_columns {
            let _ = self.recv_packet().await?;
        }

        self.maybe_recv_eof().await?;

        Ok(())
    }

    pub fn boxed_socket(self) -> MySqlStream {
        MySqlStream {
            socket: self.socket.boxed(),
            server_version: self.server_version,
            capabilities: self.capabilities,
            sequence_id: self.sequence_id,
            waiting: self.waiting,
            charset: self.charset,
            collation: self.collation,
            is_tls: self.is_tls,
            connection_id: self.connection_id,
            observed_status: self.observed_status,
            current_database: self.current_database,
            database_confirmed: self.database_confirmed,
            schema_tracking_enabled: self.schema_tracking_enabled,
            system_tracking_all: self.system_tracking_all,
            context_tracking_initialized: self.context_tracking_initialized,
        }
    }
}

impl<S> Deref for MySqlStream<S> {
    type Target = BufferedSocket<S>;

    fn deref(&self) -> &Self::Target {
        &self.socket
    }
}

impl<S> DerefMut for MySqlStream<S> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.socket
    }
}
