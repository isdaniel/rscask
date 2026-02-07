use clap::Parser;
use rscask::{Bitcask, Error, Request, Response, Result};
use std::io::{Read, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::PathBuf;

#[derive(Parser)]
#[command(name = "rscaskd", version, about = "Bitcask daemon")]
struct Cli {
    #[arg(long, default_value = "./db")]
    db: PathBuf,
    #[arg(long, default_value = "/tmp/rscask.sock")]
    socket: PathBuf,
}

fn main() {
    if let Err(err) = run() {
        eprintln!("{err}");
        std::process::exit(1);
    }
}

fn run() -> Result<()> {
    let cli = Cli::parse();
    if cli.socket.exists() {
        std::fs::remove_file(&cli.socket)?;
    }
    let listener = UnixListener::bind(&cli.socket)?;
    let mut store = Bitcask::open(cli.db)?;
    for stream in listener.incoming() {
        let mut stream = stream?;
        let request = read_request(&mut stream)?;
        let should_shutdown = matches!(request, Request::Shutdown);
        let response = handle_request(&mut store, request)?;
        write_response(&mut stream, &response)?;
        if should_shutdown {
            break;
        }
    }
    Ok(())
}

fn handle_request(
    store: &mut Bitcask<rscask::storage::FileLogStore, rscask::keydir::HashMapKeyDir>,
    request: Request,
) -> Result<Response> {
    match request {
        Request::Insert { key, value } => {
            if store.contains_key(&key) {
                return Ok(Response::Error("key already exists".to_string()));
            }
            store.put(&key, &value)?;
            Ok(Response::Ok)
        }
        Request::Update { key, value } => {
            if !store.contains_key(&key) {
                return Ok(Response::Error("key not found".to_string()));
            }
            store.put(&key, &value)?;
            Ok(Response::Ok)
        }
        Request::Delete { key } => {
            if !store.delete(&key)? {
                return Ok(Response::Error("key not found".to_string()));
            }
            Ok(Response::Ok)
        }
        Request::Get { key } => {
            let value = store.get(&key)?;
            Ok(Response::Value(value))
        }
        Request::Merge => {
            store.merge()?;
            Ok(Response::Ok)
        }
        Request::Stats => Ok(Response::Stats { keys: store.len() }),
        Request::Shutdown => Ok(Response::Ok),
    }
}

fn read_request(stream: &mut UnixStream) -> Result<Request> {
    let mut len_buf = [0u8; 4];
    stream.read_exact(&mut len_buf)?;
    let len = u32::from_be_bytes(len_buf) as usize;
    let mut buf = vec![0u8; len];
    stream.read_exact(&mut buf)?;
    let (request, _read) =
        bincode::serde::decode_from_slice(&buf, bincode::config::standard())
            .map_err(|err| Error::Invalid(err.to_string()))?;
    Ok(request)
}

fn write_response(stream: &mut UnixStream, response: &Response) -> Result<()> {
    let payload = bincode::serde::encode_to_vec(response, bincode::config::standard())
        .map_err(|err| Error::Invalid(err.to_string()))?;
    let len = (payload.len() as u32).to_be_bytes();
    stream.write_all(&len)?;
    stream.write_all(&payload)?;
    stream.flush()?;
    Ok(())
}
