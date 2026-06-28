//! End-to-end transport test: two threads over a real named socket perform the
//! handshake, then the caller issues an `add_numbers` request which the provider
//! serves, returning `5`.

use std::thread;
use std::time::{SystemTime, UNIX_EPOCH};

use napi_oop::codec::{read_message, write_message, Hello, Message, Request, Response, Role};
use napi_oop::peer::handshake;
use napi_oop::transport::{connect, listen};
use napi_oop::PROTOCOL_VERSION;

fn unique_socket_path() -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let dir = std::env::temp_dir();
    dir.join(format!("napi-oop-test-{}-{}.sock", std::process::id(), nanos))
        .to_string_lossy()
        .into_owned()
}

#[test]
fn handshake_and_add_numbers_round_trip() {
    let path = unique_socket_path();
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

        let _ = std::fs::remove_file(&provider_path);
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
