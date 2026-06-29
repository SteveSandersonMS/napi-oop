//! End-to-end transport test: two threads over a real named socket perform the
//! handshake, then the caller issues an `add_numbers` request which the provider
//! serves, returning `5`.

use std::thread;

use napi_oop::bootstrap::{cleanup_socket_path, generate_socket_path};
use napi_oop::codec::{read_message, write_message, Hello, Message, Request, Response, Role};
use napi_oop::peer::handshake;
use napi_oop::transport::{connect, listen};
use napi_oop::PROTOCOL_VERSION;

#[test]
fn handshake_and_add_numbers_round_trip() {
    let path = generate_socket_path();
    let listener = listen(&path).expect("listen");

    let provider_path = path.clone();
    let provider = thread::spawn(move || {
        let mut stream = listener.accept().expect("accept");
        let peer = handshake(
            &mut stream,
            Hello {
                version: PROTOCOL_VERSION,
                role: Role::Provider,
                functions: vec!["add_numbers".into()],
            },
        )
        .expect("provider handshake");
        assert_eq!(peer.role, Role::Caller);

        // Serve exactly one request.
        let req = match read_message(&mut stream).expect("read request") {
            Some(Message::Request(req)) => req,
            other => panic!("expected request, got {other:?}"),
        };
        assert_eq!(req.function, "add_numbers");
        let a = req.args[0].as_i64().unwrap();
        let b = req.args[1].as_i64().unwrap();
        write_message(
            &mut stream,
            &Message::Response(Response {
                id: req.id,
                result: rmpv::Value::from(a + b),
            }),
        )
        .expect("write response");

        cleanup_socket_path(&provider_path);
    });

    let mut stream = connect(&path).expect("connect");
    let peer = handshake(
        &mut stream,
        Hello {
            version: PROTOCOL_VERSION,
            role: Role::Caller,
            functions: vec![],
        },
    )
    .expect("caller handshake");
    assert_eq!(peer.role, Role::Provider);
    assert_eq!(peer.functions, vec!["add_numbers".to_string()]);

    write_message(
        &mut stream,
        &Message::Request(Request {
            id: 1,
            function: "add_numbers".into(),
            args: vec![rmpv::Value::from(2i64), rmpv::Value::from(3i64)],
        }),
    )
    .expect("write request");

    let resp = match read_message(&mut stream).expect("read response") {
        Some(Message::Response(resp)) => resp,
        other => panic!("expected response, got {other:?}"),
    };
    assert_eq!(resp.id, 1);
    assert_eq!(resp.result.as_i64(), Some(5));

    provider.join().unwrap();
}
