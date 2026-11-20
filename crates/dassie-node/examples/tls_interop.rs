//! One TLS 1.3 handshake on the node's crypto provider (`tls::provider()`),
//! for the interop test against OpenSSL (tests/interop/tls_interop_test.ts):
//!
//! ```text
//! tls_interop serve <tls dir>
//! tls_interop connect <port> <tls dir>
//! ```
//!
//! `<tls dir>` holds the tests' certificate (testdata/tls). `serve` listens
//! on a port of loopback, prints it, takes one connection, writes the name
//! of the group the handshake agreed on, and exits. `connect` dials
//! `localhost:<port>`, prints that name, then what the server sent, and
//! exits.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]
// A process for the interop test: it prints what it agreed on.
#![allow(clippy::disallowed_methods, clippy::disallowed_macros)]

use std::io::{Read as _, Write as _};
use std::net::{TcpListener, TcpStream};
use std::sync::Arc;

use dassie_node::tls;
use rustls::pki_types::pem::PemObject as _;
use rustls::pki_types::{CertificateDer, PrivateKeyDer};
use rustls::{ClientConfig, ClientConnection, RootCertStore, ServerConfig, ServerConnection};

fn group(connection: &rustls::CommonState) -> String {
    format!(
        "{:?}",
        connection
            .negotiated_key_exchange_group()
            .expect("the handshake is done")
            .name()
    )
}

fn serve(dir: &str) {
    let chain = CertificateDer::pem_file_iter(format!("{dir}/cert.pem"))
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    let key = PrivateKeyDer::from_pem_file(format!("{dir}/key.pem")).unwrap();
    let config = ServerConfig::builder_with_provider(tls::provider())
        .with_protocol_versions(&[&rustls::version::TLS13])
        .unwrap()
        .with_no_client_auth()
        .with_single_cert(chain, key)
        .unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    println!("{}", listener.local_addr().unwrap().port());
    let (mut socket, _) = listener.accept().unwrap();
    let mut connection = ServerConnection::new(Arc::new(config)).unwrap();
    while connection.is_handshaking() {
        connection.complete_io(&mut socket).unwrap();
    }
    let group = group(&connection);
    println!("{group}");
    let mut stream = rustls::Stream::new(&mut connection, &mut socket);
    stream.write_all(format!("{group}\n").as_bytes()).unwrap();
    stream.conn.send_close_notify();
    stream.flush().unwrap();
}

fn connect(port: u16, dir: &str) {
    let mut roots = RootCertStore::empty();
    for root in CertificateDer::pem_file_iter(format!("{dir}/root.pem")).unwrap() {
        roots.add(root.unwrap()).unwrap();
    }
    let config = ClientConfig::builder_with_provider(tls::provider())
        .with_protocol_versions(&[&rustls::version::TLS13])
        .unwrap()
        .with_root_certificates(roots)
        .with_no_client_auth();
    let mut socket = TcpStream::connect(("127.0.0.1", port)).unwrap();
    let mut connection =
        ClientConnection::new(Arc::new(config), "localhost".try_into().unwrap()).unwrap();
    while connection.is_handshaking() {
        connection.complete_io(&mut socket).unwrap();
    }
    println!("{}", group(&connection));
    let mut answer = String::new();
    rustls::Stream::new(&mut connection, &mut socket)
        .read_to_string(&mut answer)
        .unwrap();
    print!("{answer}");
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    match args.get(1).map(String::as_str) {
        Some("serve") => serve(&args[2]),
        Some("connect") => connect(args[2].parse().unwrap(), &args[3]),
        _ => panic!("usage: tls_interop serve <tls dir> | connect <port> <tls dir>"),
    }
}
