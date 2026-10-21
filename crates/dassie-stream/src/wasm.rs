//! JavaScript bindings (wasm-bindgen) for the STREAM state machines, used
//! by the TypeScript driver in `packages/identity-wasm`, the interop tests
//! and the website's STREAM toy.
//!
//! The JavaScript side owns the clock and the network: it passes the time
//! in milliseconds and moves packets as `Uint8Array`s. Randomness comes
//! from the platform here, at the edge (spec/decisions/tigerstyle.md).

use alloc::string::{String, ToString};
use alloc::vec;
use alloc::vec::Vec;

use dassie_ilp::{Address, ErrorCode as IlpErrorCode, Prepare, Reject, Timestamp};
use dassie_oer::{Decode, Encode};
use rand_core::UnwrapErr;
use wasm_bindgen::prelude::*;

use crate::connection::{AssetDetails, BUFFER_LENGTH, Config, Connection, Role};
use crate::crypto::Keys;
use crate::event::Event;
use crate::frame::{ErrorCode, Frame};
use crate::packet::Packet;
use crate::policy::Policy;
use crate::rate::Rate;
use crate::receipt::ReceiptSecret;
use crate::server::{ConnectionId, Server, ServerConfig};

// The bindings are the edge where the browser supplies randomness.
#[allow(clippy::disallowed_types)]
fn rng() -> UnwrapErr<getrandom::SysRng> {
    UnwrapErr(getrandom::SysRng)
}

fn time(now_ms: f64) -> Result<Timestamp, JsError> {
    if !(0.0..=253_402_300_799_999.0).contains(&now_ms) {
        return Err(JsError::new("time out of range"));
    }
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    // Checked to be in range above.
    let unix_ms = now_ms as u64;
    Ok(Timestamp::from_unix_ms(unix_ms))
}

fn address(text: &str) -> Result<Address<'_>, JsError> {
    Address::parse(text).map_err(|error| JsError::new(&error.to_string()))
}

fn stream_error(error: crate::connection::StreamError) -> JsError {
    JsError::new(&alloc::format!("{error:?}"))
}

/// What happened, for JavaScript. Fields an event kind lacks are zero.
#[wasm_bindgen]
pub struct StreamEvent {
    kind: &'static str,
    stream_id: u64,
    code: u8,
    by_remote: bool,
    first: u64,
    second: u64,
}

#[wasm_bindgen]
impl StreamEvent {
    /// `connected`, `streamOpened`, `moneyReceived`, `moneySent`,
    /// `dataReadable`, `streamClosed`, `exchangeRateMeasured`,
    /// `exchangeRateMeasurementFailed`, `exchangeRateTooLow` or
    /// `connectionClosed`.
    #[wasm_bindgen(getter)]
    #[must_use]
    pub fn kind(&self) -> String {
        String::from(self.kind)
    }

    /// The stream, for stream events.
    #[wasm_bindgen(getter, js_name = streamId)]
    #[must_use]
    pub fn stream_id(&self) -> u64 {
        self.stream_id
    }

    /// The STREAM error code of a close.
    #[wasm_bindgen(getter)]
    #[must_use]
    pub fn code(&self) -> u8 {
        self.code
    }

    /// Whether the remote closed the connection.
    #[wasm_bindgen(getter, js_name = byRemote)]
    #[must_use]
    pub fn by_remote(&self) -> bool {
        self.by_remote
    }

    /// Total sent of a closed stream, a rate's numerator, or the expected
    /// amount of a too-low rate.
    #[wasm_bindgen(getter)]
    #[must_use]
    pub fn first(&self) -> u64 {
        self.first
    }

    /// Total received of a closed stream, a rate's denominator, or the
    /// received amount of a too-low rate.
    #[wasm_bindgen(getter)]
    #[must_use]
    pub fn second(&self) -> u64 {
        self.second
    }
}

impl From<Event> for StreamEvent {
    fn from(event: Event) -> Self {
        let mut view = Self {
            kind: "connected",
            stream_id: 0,
            code: 0,
            by_remote: false,
            first: 0,
            second: 0,
        };
        match event {
            Event::Connected => {}
            Event::StreamOpened { stream_id } => {
                (view.kind, view.stream_id) = ("streamOpened", stream_id)
            }
            Event::MoneyReceived { stream_id } => {
                (view.kind, view.stream_id) = ("moneyReceived", stream_id)
            }
            Event::MoneySent { stream_id } => {
                (view.kind, view.stream_id) = ("moneySent", stream_id)
            }
            Event::DataReadable { stream_id } => {
                (view.kind, view.stream_id) = ("dataReadable", stream_id)
            }
            Event::StreamClosed {
                stream_id,
                code,
                total_sent,
                total_received,
            } => {
                view.kind = "streamClosed";
                view.stream_id = stream_id;
                view.code = code.0;
                (view.first, view.second) = (total_sent, total_received);
            }
            Event::ExchangeRateMeasured { rate } => {
                view.kind = "exchangeRateMeasured";
                (view.first, view.second) = (rate.numerator(), rate.denominator());
            }
            Event::ExchangeRateMeasurementFailed => view.kind = "exchangeRateMeasurementFailed",
            Event::ExchangeRateTooLow { expected, received } => {
                view.kind = "exchangeRateTooLow";
                (view.first, view.second) = (expected, received);
            }
            Event::ConnectionClosed { code, by_remote } => {
                view.kind = "connectionClosed";
                view.code = code.0;
                view.by_remote = by_remote;
            }
        }
        view
    }
}

/// An outgoing ILP Prepare and the ID to answer it with.
#[wasm_bindgen]
pub struct StreamTransmit {
    request_id: u64,
    packet: Vec<u8>,
    expires_at: f64,
}

#[wasm_bindgen]
impl StreamTransmit {
    /// Pass this back with the response.
    #[wasm_bindgen(getter, js_name = requestId)]
    #[must_use]
    pub fn request_id(&self) -> u64 {
        self.request_id
    }

    /// The ILP Prepare.
    #[wasm_bindgen(getter)]
    #[must_use]
    pub fn packet(&self) -> Vec<u8> {
        self.packet.clone()
    }

    /// When it expires, in milliseconds since the epoch.
    #[wasm_bindgen(getter, js_name = expiresAt)]
    #[must_use]
    pub fn expires_at(&self) -> f64 {
        self.expires_at
    }
}

/// The STREAM operations common to client and server connections.
struct Driver {
    buffer: Vec<u8>,
}

impl Driver {
    fn new() -> Self {
        Self {
            buffer: vec![0; BUFFER_LENGTH],
        }
    }

    fn transmit(
        &mut self,
        connection: &mut Connection,
        now_ms: f64,
    ) -> Result<Option<StreamTransmit>, JsError> {
        let now = time(now_ms)?;
        let Some(transmit) = connection.poll_transmit(now, &mut rng(), &mut self.buffer) else {
            return Ok(None);
        };
        #[allow(clippy::cast_precision_loss)]
        // Interledger timestamps fit an f64 exactly.
        let expires_at = transmit.expires_at.unix_ms as f64;
        Ok(Some(StreamTransmit {
            request_id: transmit.request_id,
            packet: self
                .buffer
                .get(..transmit.length)
                .unwrap_or_default()
                .to_vec(),
            expires_at,
        }))
    }

    fn respond(
        &mut self,
        connection: &mut Connection,
        now_ms: f64,
        request_id: u64,
        response: &[u8],
    ) -> Result<(), JsError> {
        connection.handle_response(time(now_ms)?, request_id, response, &mut self.buffer);
        Ok(())
    }
}

/// Answers a packet that is not a valid Prepare with F01 (ILP-3).
fn invalid_packet(triggered_by: Option<Address<'_>>) -> Vec<u8> {
    Reject {
        code: IlpErrorCode::F01_INVALID_PACKET,
        triggered_by,
        message: "",
        data: &[],
    }
    .to_vec()
}

fn read(
    connection: &mut Connection,
    stream_id: u64,
    max_length: usize,
) -> Result<Vec<u8>, JsError> {
    let mut output = vec![0; max_length.min(1 << 20)];
    let count = connection
        .read(stream_id, &mut output)
        .map_err(stream_error)?;
    output.truncate(count);
    Ok(output)
}

/// A STREAM client connection.
#[wasm_bindgen]
pub struct StreamConnection {
    connection: Connection,
    driver: Driver,
}

#[wasm_bindgen]
impl StreamConnection {
    /// Connects to `destination` with the shared secret from the server.
    /// Without a local address the connection can send but not receive.
    #[wasm_bindgen(constructor)]
    pub fn new(
        shared_secret: &[u8],
        destination: &str,
        local_address: Option<String>,
        asset_code: Option<String>,
        asset_scale: u8,
    ) -> Result<StreamConnection, JsError> {
        let local = match local_address.as_deref() {
            Some(text) => Some(address(text)?),
            None => None,
        };
        let connection = Connection::new(&Config {
            role: Role::Client,
            shared_secret,
            local_address: local,
            remote_address: Some(address(destination)?),
            local_asset: asset_code.as_deref().map(|code| AssetDetails {
                code,
                scale: asset_scale,
            }),
            receipt: None,
            policy: Policy::DEFAULT,
        });
        Ok(Self {
            connection,
            driver: Driver::new(),
        })
    }

    /// Opens a stream and returns its ID.
    #[wasm_bindgen(js_name = openStream)]
    pub fn open_stream(&mut self) -> Result<u64, JsError> {
        self.connection.open_stream().map_err(stream_error)
    }

    /// Sets how much a stream sends in total.
    #[wasm_bindgen(js_name = setSendMax)]
    pub fn set_send_max(&mut self, stream_id: u64, amount: u64) -> Result<(), JsError> {
        self.connection
            .set_send_max(stream_id, amount)
            .map_err(stream_error)
    }

    /// Sets how much a stream accepts in total.
    #[wasm_bindgen(js_name = setReceiveMax)]
    pub fn set_receive_max(&mut self, stream_id: u64, amount: u64) -> Result<(), JsError> {
        self.connection
            .set_receive_max(stream_id, amount)
            .map_err(stream_error)
    }

    /// Queues data and returns how much fit.
    pub fn write(&mut self, stream_id: u64, data: &[u8]) -> Result<usize, JsError> {
        self.connection.write(stream_id, data).map_err(stream_error)
    }

    /// Reads up to `max_length` bytes of in-order data.
    pub fn read(&mut self, stream_id: u64, max_length: usize) -> Result<Vec<u8>, JsError> {
        read(&mut self.connection, stream_id, max_length)
    }

    /// Closes a stream with a STREAM error code (1 for no error).
    #[wasm_bindgen(js_name = closeStream)]
    pub fn close_stream(&mut self, stream_id: u64, code: u8) -> Result<(), JsError> {
        self.connection
            .close_stream(stream_id, ErrorCode(code))
            .map_err(stream_error)
    }

    /// What a stream has sent and the receiver acknowledged.
    #[wasm_bindgen(js_name = totalSent)]
    #[must_use]
    pub fn total_sent(&self, stream_id: u64) -> u64 {
        self.connection
            .stream(stream_id)
            .map_or(0, crate::Stream::total_sent)
    }

    /// What a stream has received.
    #[wasm_bindgen(js_name = totalReceived)]
    #[must_use]
    pub fn total_received(&self, stream_id: u64) -> u64 {
        self.connection
            .stream(stream_id)
            .map_or(0, crate::Stream::total_received)
    }

    /// The latest receipt for a stream, if the receiver issues them.
    #[must_use]
    pub fn receipt(&self, stream_id: u64) -> Option<Vec<u8>> {
        self.connection
            .stream(stream_id)
            .and_then(crate::Stream::last_receipt)
            .map(|receipt| receipt.to_vec())
    }

    /// Sets the minimum exchange rate: `numerator` destination units per
    /// `denominator` source units.
    #[wasm_bindgen(js_name = setExchangeRate)]
    pub fn set_exchange_rate(&mut self, numerator: u64, denominator: u64) -> Result<(), JsError> {
        let rate =
            Rate::new(numerator, denominator).ok_or_else(|| JsError::new("zero denominator"))?;
        self.connection.set_exchange_rate(rate);
        Ok(())
    }

    /// Measures the exchange rate; the result arrives as an event.
    #[wasm_bindgen(js_name = measureExchangeRate)]
    pub fn measure_exchange_rate(&mut self) {
        self.connection.measure_exchange_rate();
    }

    /// Sends without caring how much arrives.
    #[wasm_bindgen(js_name = dangerouslyIgnoreExchangeRate)]
    pub fn dangerously_ignore_exchange_rate(&mut self) {
        self.connection.dangerously_ignore_exchange_rate();
    }

    /// Starts closing the connection.
    pub fn close(&mut self, now_ms: f64, code: u8) -> Result<(), JsError> {
        self.connection.close(time(now_ms)?, ErrorCode(code));
        Ok(())
    }

    /// Whether the connection is closed.
    #[wasm_bindgen(js_name = isClosed)]
    #[must_use]
    pub fn is_closed(&self) -> bool {
        self.connection.is_closed()
    }

    /// The next packet to send, if any.
    #[wasm_bindgen(js_name = pollTransmit)]
    pub fn poll_transmit(&mut self, now_ms: f64) -> Result<Option<StreamTransmit>, JsError> {
        self.driver.transmit(&mut self.connection, now_ms)
    }

    /// The response to a packet from `pollTransmit`.
    #[wasm_bindgen(js_name = handleResponse)]
    pub fn handle_response(
        &mut self,
        now_ms: f64,
        request_id: u64,
        response: &[u8],
    ) -> Result<(), JsError> {
        self.driver
            .respond(&mut self.connection, now_ms, request_id, response)
    }

    /// Answers an incoming ILP Prepare.
    #[wasm_bindgen(js_name = handlePrepare)]
    pub fn handle_prepare(&mut self, now_ms: f64, prepare: &[u8]) -> Result<Vec<u8>, JsError> {
        let now = time(now_ms)?;
        let Ok(prepare) = Prepare::decode_exact(prepare) else {
            return Ok(invalid_packet(None));
        };
        let length =
            self.connection
                .handle_prepare(now, &mut rng(), &prepare, &mut self.driver.buffer);
        Ok(self
            .driver
            .buffer
            .get(..length)
            .unwrap_or_default()
            .to_vec())
    }

    /// Expires what is due.
    #[wasm_bindgen(js_name = handleTimeout)]
    pub fn handle_timeout(&mut self, now_ms: f64) -> Result<(), JsError> {
        self.connection.handle_timeout(time(now_ms)?);
        Ok(())
    }

    /// When `handleTimeout` should next run, in milliseconds since the
    /// epoch.
    #[wasm_bindgen(js_name = pollTimeout)]
    #[must_use]
    pub fn poll_timeout(&self) -> Option<f64> {
        #[allow(clippy::cast_precision_loss)]
        self.connection
            .poll_timeout()
            .map(|time| time.unix_ms as f64)
    }

    /// The next event, if any.
    #[wasm_bindgen(js_name = pollEvent)]
    pub fn poll_event(&mut self) -> Option<StreamEvent> {
        self.connection.poll_event().map(StreamEvent::from)
    }
}

/// Credentials for a client.
#[wasm_bindgen]
pub struct StreamCredentials {
    destination: String,
    shared_secret: Vec<u8>,
}

#[wasm_bindgen]
impl StreamCredentials {
    /// The ILP address to pay.
    #[wasm_bindgen(getter)]
    #[must_use]
    pub fn destination(&self) -> String {
        self.destination.clone()
    }

    /// The shared secret.
    #[wasm_bindgen(getter, js_name = sharedSecret)]
    #[must_use]
    pub fn shared_secret(&self) -> Vec<u8> {
        self.shared_secret.clone()
    }
}

/// A STREAM server. Connection methods take the connection's ID, as
/// reported by `pollOpened`.
#[wasm_bindgen]
pub struct StreamServer {
    server: Server,
    driver: Driver,
}

impl StreamServer {
    fn connection(&mut self, id: u64) -> Result<&mut Connection, JsError> {
        self.server
            .connection_mut(ConnectionId(id))
            .ok_or_else(|| JsError::new("unknown connection"))
    }
}

#[wasm_bindgen]
impl StreamServer {
    /// A server at `address` with a 32-byte secret. Streams that clients
    /// open accept up to `stream_receive_max` until changed.
    #[wasm_bindgen(constructor)]
    pub fn new(
        secret: &[u8],
        server_address: &str,
        asset_code: &str,
        asset_scale: u8,
        stream_receive_max: u64,
    ) -> Result<StreamServer, JsError> {
        let secret: [u8; 32] = secret
            .try_into()
            .map_err(|_| JsError::new("the server secret has 32 bytes"))?;
        let server = Server::new(&ServerConfig {
            secret,
            address: address(server_address)?,
            asset: Some(AssetDetails {
                code: asset_code,
                scale: asset_scale,
            }),
            policy: Policy {
                remote_stream_receive_max: stream_receive_max,
                ..Policy::DEFAULT
            },
            max_connections: 64,
        });
        Ok(Self {
            server,
            driver: Driver::new(),
        })
    }

    /// Issues credentials, with receipts if both the 16-byte nonce and the
    /// 32-byte secret are given.
    #[wasm_bindgen(js_name = generateCredentials)]
    pub fn generate_credentials(
        &self,
        connection_tag: &[u8],
        receipt_nonce: Option<Vec<u8>>,
        receipt_secret: Option<Vec<u8>>,
    ) -> Result<StreamCredentials, JsError> {
        let receipt = match (receipt_nonce, receipt_secret) {
            (Some(nonce), Some(secret)) => Some(ReceiptSecret {
                nonce: nonce
                    .try_into()
                    .map_err(|_| JsError::new("the nonce has 16 bytes"))?,
                secret: secret
                    .try_into()
                    .map_err(|_| JsError::new("the secret has 32 bytes"))?,
            }),
            (None, None) => None,
            _ => return Err(JsError::new("receipts need both nonce and secret")),
        };
        if connection_tag.len() > crate::token::MAX_CONNECTION_TAG_LENGTH {
            return Err(JsError::new("connection tag too long"));
        }
        let credentials = self
            .server
            .generate_credentials(&mut rng(), connection_tag, receipt);
        Ok(StreamCredentials {
            destination: credentials.destination,
            shared_secret: credentials.shared_secret.to_vec(),
        })
    }

    /// Answers an incoming ILP Prepare.
    #[wasm_bindgen(js_name = handlePrepare)]
    pub fn handle_prepare(&mut self, now_ms: f64, prepare: &[u8]) -> Result<Vec<u8>, JsError> {
        let now = time(now_ms)?;
        let Ok(prepare) = Prepare::decode_exact(prepare) else {
            return Ok(invalid_packet(Address::parse(self.server.address()).ok()));
        };
        let length = self
            .server
            .handle_prepare(now, &mut rng(), &prepare, &mut self.driver.buffer);
        Ok(self
            .driver
            .buffer
            .get(..length)
            .unwrap_or_default()
            .to_vec())
    }

    /// The next newly opened connection's ID.
    #[wasm_bindgen(js_name = pollOpened)]
    pub fn poll_opened(&mut self) -> Option<u64> {
        self.server.poll_opened().map(|id| id.0)
    }

    /// The tag a connection's credentials carried.
    #[wasm_bindgen(js_name = connectionTag)]
    #[must_use]
    pub fn connection_tag(&self, id: u64) -> Option<Vec<u8>> {
        self.server
            .connection_tag(ConnectionId(id))
            .map(<[u8]>::to_vec)
    }

    /// The next event of a connection.
    #[wasm_bindgen(js_name = pollEvent)]
    pub fn poll_event(&mut self, id: u64) -> Result<Option<StreamEvent>, JsError> {
        Ok(self.connection(id)?.poll_event().map(StreamEvent::from))
    }

    /// Sets how much a stream accepts in total.
    #[wasm_bindgen(js_name = setReceiveMax)]
    pub fn set_receive_max(&mut self, id: u64, stream_id: u64, amount: u64) -> Result<(), JsError> {
        self.connection(id)?
            .set_receive_max(stream_id, amount)
            .map_err(stream_error)
    }

    /// What a stream has received.
    #[wasm_bindgen(js_name = totalReceived)]
    pub fn total_received(&mut self, id: u64, stream_id: u64) -> Result<u64, JsError> {
        Ok(self
            .connection(id)?
            .stream(stream_id)
            .map_or(0, crate::Stream::total_received))
    }

    /// Reads up to `max_length` bytes of in-order data from a stream.
    pub fn read(&mut self, id: u64, stream_id: u64, max_length: usize) -> Result<Vec<u8>, JsError> {
        read(self.connection(id)?, stream_id, max_length)
    }

    /// Queues data on a stream and returns how much fit.
    pub fn write(&mut self, id: u64, stream_id: u64, data: &[u8]) -> Result<usize, JsError> {
        self.connection(id)?
            .write(stream_id, data)
            .map_err(stream_error)
    }

    /// Opens a stream on a connection.
    #[wasm_bindgen(js_name = openStream)]
    pub fn open_stream(&mut self, id: u64) -> Result<u64, JsError> {
        self.connection(id)?.open_stream().map_err(stream_error)
    }

    /// Sets how much a stream sends in total.
    #[wasm_bindgen(js_name = setSendMax)]
    pub fn set_send_max(&mut self, id: u64, stream_id: u64, amount: u64) -> Result<(), JsError> {
        self.connection(id)?
            .set_send_max(stream_id, amount)
            .map_err(stream_error)
    }

    /// Sets a connection's minimum exchange rate.
    #[wasm_bindgen(js_name = setExchangeRate)]
    pub fn set_exchange_rate(
        &mut self,
        id: u64,
        numerator: u64,
        denominator: u64,
    ) -> Result<(), JsError> {
        let rate =
            Rate::new(numerator, denominator).ok_or_else(|| JsError::new("zero denominator"))?;
        self.connection(id)?.set_exchange_rate(rate);
        Ok(())
    }

    /// The next packet a connection sends, if any.
    #[wasm_bindgen(js_name = pollTransmit)]
    pub fn poll_transmit(
        &mut self,
        id: u64,
        now_ms: f64,
    ) -> Result<Option<StreamTransmit>, JsError> {
        let connection = self
            .server
            .connection_mut(ConnectionId(id))
            .ok_or_else(|| JsError::new("unknown connection"))?;
        self.driver.transmit(connection, now_ms)
    }

    /// The response to a packet a connection sent.
    #[wasm_bindgen(js_name = handleResponse)]
    pub fn handle_response(
        &mut self,
        id: u64,
        now_ms: f64,
        request_id: u64,
        response: &[u8],
    ) -> Result<(), JsError> {
        let connection = self
            .server
            .connection_mut(ConnectionId(id))
            .ok_or_else(|| JsError::new("unknown connection"))?;
        self.driver
            .respond(connection, now_ms, request_id, response)
    }

    /// Expires what is due on every connection.
    #[wasm_bindgen(js_name = handleTimeout)]
    pub fn handle_timeout(&mut self, now_ms: f64) -> Result<(), JsError> {
        let now = time(now_ms)?;
        let ids: Vec<ConnectionId> = self.server.connections().map(|(id, _)| id).collect();
        for id in ids {
            if let Some(connection) = self.server.connection_mut(id) {
                connection.handle_timeout(now);
            }
        }
        Ok(())
    }

    /// The earliest deadline of any connection.
    #[wasm_bindgen(js_name = pollTimeout)]
    #[must_use]
    pub fn poll_timeout(&self) -> Option<f64> {
        #[allow(clippy::cast_precision_loss)]
        self.server
            .connections()
            .filter_map(|(_, connection)| connection.poll_timeout())
            .min()
            .map(|time| time.unix_ms as f64)
    }
}

/// A decoded STREAM frame, for display. Fields a frame type lacks are zero
/// or empty.
#[wasm_bindgen]
#[derive(Clone)]
pub struct StreamFrameView {
    kind: &'static str,
    stream_id: u64,
    code: u8,
    text: String,
    first: u64,
    second: u64,
    length: usize,
}

#[wasm_bindgen]
impl StreamFrameView {
    /// The frame type as RFC 29 names it, in camel case: `streamMoney`,
    /// `connectionClose` and so on.
    #[wasm_bindgen(getter)]
    #[must_use]
    pub fn kind(&self) -> String {
        self.kind.to_string()
    }

    /// The stream of a `stream*` frame.
    #[wasm_bindgen(getter, js_name = streamId)]
    #[must_use]
    pub fn stream_id(&self) -> u64 {
        self.stream_id
    }

    /// The error code of a close frame.
    #[wasm_bindgen(getter)]
    #[must_use]
    pub fn code(&self) -> u8 {
        self.code
    }

    /// The message of a close frame, the address of `connectionNewAddress`,
    /// or the asset code of `connectionAssetDetails`.
    #[wasm_bindgen(getter)]
    #[must_use]
    pub fn text(&self) -> String {
        self.text.clone()
    }

    /// The frame's first number: shares, an offset, a maximum, or the asset
    /// scale.
    #[wasm_bindgen(getter)]
    #[must_use]
    pub fn first(&self) -> u64 {
        self.first
    }

    /// The frame's second number: what was received or sent so far.
    #[wasm_bindgen(getter)]
    #[must_use]
    pub fn second(&self) -> u64 {
        self.second
    }

    /// How many bytes a `streamData` or `streamReceipt` frame carries.
    #[wasm_bindgen(getter)]
    #[must_use]
    pub fn length(&self) -> usize {
        self.length
    }
}

impl StreamFrameView {
    fn new(kind: &'static str, stream_id: u64, first: u64, second: u64) -> Self {
        Self {
            kind,
            stream_id,
            code: 0,
            text: String::new(),
            first,
            second,
            length: 0,
        }
    }
}

impl From<Frame<'_>> for StreamFrameView {
    fn from(frame: Frame<'_>) -> Self {
        let new = Self::new;
        match frame {
            Frame::ConnectionClose { code, message } => Self {
                code: code.0,
                text: message.into(),
                ..new("connectionClose", 0, 0, 0)
            },
            Frame::ConnectionNewAddress { source_account } => Self {
                text: source_account.map_or_else(String::new, |a| a.as_str().into()),
                ..new("connectionNewAddress", 0, 0, 0)
            },
            Frame::ConnectionMaxData { max_offset } => new("connectionMaxData", 0, max_offset, 0),
            Frame::ConnectionDataBlocked { max_offset } => {
                new("connectionDataBlocked", 0, max_offset, 0)
            }
            Frame::ConnectionMaxStreamId { max_stream_id } => {
                new("connectionMaxStreamId", 0, max_stream_id, 0)
            }
            Frame::ConnectionStreamIdBlocked { max_stream_id } => {
                new("connectionStreamIdBlocked", 0, max_stream_id, 0)
            }
            Frame::ConnectionAssetDetails {
                asset_code,
                asset_scale,
            } => Self {
                text: asset_code.into(),
                ..new("connectionAssetDetails", 0, u64::from(asset_scale), 0)
            },
            Frame::StreamClose {
                stream_id,
                code,
                message,
            } => Self {
                code: code.0,
                text: message.into(),
                ..new("streamClose", stream_id, 0, 0)
            },
            Frame::StreamMoney { stream_id, shares } => new("streamMoney", stream_id, shares, 0),
            Frame::StreamMaxMoney {
                stream_id,
                receive_max,
                total_received,
            } => new("streamMaxMoney", stream_id, receive_max, total_received),
            Frame::StreamMoneyBlocked {
                stream_id,
                send_max,
                total_sent,
            } => new("streamMoneyBlocked", stream_id, send_max, total_sent),
            Frame::StreamData {
                stream_id,
                offset,
                data,
            } => Self {
                length: data.len(),
                ..new("streamData", stream_id, offset, 0)
            },
            Frame::StreamMaxData {
                stream_id,
                max_offset,
            } => new("streamMaxData", stream_id, max_offset, 0),
            Frame::StreamDataBlocked {
                stream_id,
                max_offset,
            } => new("streamDataBlocked", stream_id, max_offset, 0),
            Frame::StreamReceipt { stream_id, receipt } => Self {
                length: receipt.len(),
                ..new("streamReceipt", stream_id, 0, 0)
            },
        }
    }
}

/// A decrypted STREAM packet, for display.
#[wasm_bindgen]
pub struct StreamPacketView {
    packet_type: u8,
    sequence: u64,
    prepare_amount: u64,
    frames: Vec<StreamFrameView>,
}

#[wasm_bindgen]
impl StreamPacketView {
    /// The ILP packet type it travels on: 12 Prepare, 13 Fulfill, 14 Reject.
    #[wasm_bindgen(getter, js_name = type)]
    #[must_use]
    pub fn packet_type(&self) -> u8 {
        self.packet_type
    }

    /// Identifies a request and its response.
    #[wasm_bindgen(getter)]
    #[must_use]
    pub fn sequence(&self) -> u64 {
        self.sequence
    }

    /// In a request, the least the receiver must get; in a response, what
    /// it got.
    #[wasm_bindgen(getter, js_name = prepareAmount)]
    #[must_use]
    pub fn prepare_amount(&self) -> u64 {
        self.prepare_amount
    }

    /// The known frames in order; unknown ones are left out.
    #[wasm_bindgen(getter)]
    #[must_use]
    pub fn frames(&self) -> Vec<StreamFrameView> {
        self.frames.clone()
    }
}

/// Decrypts and parses the STREAM packet in an ILP packet's `data` with the
/// connection's shared secret. Fails on data that is not a STREAM packet of
/// that connection.
#[wasm_bindgen(js_name = decodeStreamPacket)]
pub fn decode_stream_packet(
    shared_secret: &[u8],
    data: &[u8],
) -> Result<StreamPacketView, JsError> {
    let keys = Keys::derive(shared_secret);
    let mut buffer = vec![0; data.len()];
    let plaintext = keys
        .cipher
        .decrypt(data, &mut buffer)
        .map_err(|_| JsError::new("not a STREAM packet of this connection"))?;
    let packet = Packet::parse(plaintext).map_err(|error| JsError::new(&error.to_string()))?;
    Ok(StreamPacketView {
        packet_type: packet.packet_type.octet(),
        sequence: packet.sequence,
        prepare_amount: packet.prepare_amount,
        frames: packet.frames.iter().map(StreamFrameView::from).collect(),
    })
}
