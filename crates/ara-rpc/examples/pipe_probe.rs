//! Test-only RPC transport process probe. This does not dispatch Agent commands.
//!
//! Input commands: `{"op":"echo","value":...}`, `{"op":"emit","frame":{...}}`,
//! `{"op":"negotiate","version":2}`, and `{"op":"exit"}`. Stdout carries
//! only physical JSONL frames; EOF also ends the process.

use std::io;

use ara_rpc::WireValue;
use ara_rpc::input::{InputItem, RpcInputReader};
use ara_rpc::writer::RpcOutput;

#[tokio::main]
async fn main() -> io::Result<()> {
    // Own stdin before future host initialization can start other readers.
    let mut input = RpcInputReader::new(tokio::io::stdin());
    let mut output = RpcOutput::new(tokio::io::stdout());
    output
        .write_frame(&WireValue::object(vec![
            ("type", WireValue::String("ready".into())),
            ("protocolVersion", WireValue::Number(1.0)),
            ("supportedProtocolVersions", WireValue::Array(vec![WireValue::Number(1.0), WireValue::Number(2.0)])),
            ("maxFrameBytes", WireValue::Number(1024.0 * 1024.0)),
            ("maxReassembledFrameBytes", WireValue::Number(64.0 * 1024.0 * 1024.0)),
        ]))
        .await?;
    while let Some(item) = input.read_next().await? {
        match item {
            InputItem::ParseError(message) => {
                output.write_frame(&error("parse", &message)).await?;
            }
            InputItem::Frame(command) => {
                let op = command.get("op").and_then(WireValue::as_string).and_then(|value| value.to_utf8().ok());
                match op.as_deref() {
                    Some("exit") => break,
                    Some("echo") => {
                        let mut frame = WireValue::object(vec![("type", WireValue::String("probe_echo".into()))]);
                        if let Some(value) = command.get("value") {
                            frame.insert("value", value.clone());
                        }
                        output.write_frame(&frame).await?;
                    }
                    Some("emit") => {
                        if let Some(frame) = command.get("frame").filter(|frame| frame.is_object()) {
                            output.write_frame(frame).await?;
                        } else {
                            output.write_frame(&error("emit", "frame must be an object")).await?;
                        }
                    }
                    Some("negotiate") if command.get("version").and_then(WireValue::as_number) == Some(2.0) => {
                        // Match the fixed host's ordering: emit the successful
                        // response in v1, then switch for subsequent frames.
                        output
                            .write_frame(&WireValue::object(vec![
                                ("type", WireValue::String("response".into())),
                                ("command", WireValue::String("negotiate_protocol".into())),
                                ("success", WireValue::Bool(true)),
                                ("data", WireValue::object(vec![("protocolVersion", WireValue::Number(2.0))])),
                            ]))
                            .await?;
                        output.set_protocol_version(2).expect("supported version");
                    }
                    _ => {
                        output.write_frame(&error("probe", "unknown probe command")).await?;
                    }
                }
            }
        }
    }
    Ok(())
}

fn error(command: &str, message: &str) -> WireValue {
    WireValue::object(vec![
        ("type", WireValue::String("response".into())),
        ("command", WireValue::String(command.into())),
        ("success", WireValue::Bool(false)),
        ("error", WireValue::String(message.into())),
    ])
}
