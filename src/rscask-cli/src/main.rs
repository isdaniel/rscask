use clap::{Parser, Subcommand};
use rscask::{Error, Request, Response, Result};
use std::fmt::Write as _;
use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;

#[derive(Parser)]
#[command(name = "rscask-cli", version, about = "Client for rscaskd")]
struct Cli {
    #[arg(long, default_value = "/tmp/rscask.sock")]
    socket: PathBuf,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    Insert {
        key: String,
        #[arg(value_name = "VALUE", help = "Use '-' to read from stdin")]
        value: String,
    },
    Update {
        key: String,
        #[arg(value_name = "VALUE", help = "Use '-' to read from stdin")]
        value: String,
    },
    Delete { key: String },
    Get { key: String },
    Merge,
    Stats,
    Shutdown,
}

fn main() {
    if let Err(err) = run() {
        eprintln!("{err}");
        std::process::exit(1);
    }
}

fn run() -> Result<()> {
    let cli = Cli::parse();
    let request = match cli.command {
        Command::Insert { key, value } => Request::Insert {
            key: key.into_bytes(),
            value: read_value(value)?,
        },
        Command::Update { key, value } => Request::Update {
            key: key.into_bytes(),
            value: read_value(value)?,
        },
        Command::Delete { key } => Request::Delete {
            key: key.into_bytes(),
        },
        Command::Get { key } => Request::Get {
            key: key.into_bytes(),
        },
        Command::Merge => Request::Merge,
        Command::Stats => Request::Stats,
        Command::Shutdown => Request::Shutdown,
    };

    let mut stream = UnixStream::connect(cli.socket)?;
    write_request(&mut stream, &request)?;
    let response = read_response(&mut stream)?;
    handle_response(response)
}

fn read_value(value: String) -> Result<Vec<u8>> {
    if value == "-" {
        let mut buf = Vec::new();
        std::io::stdin().read_to_end(&mut buf)?;
        Ok(buf)
    } else {
        Ok(value.into_bytes())
    }
}

fn write_request(stream: &mut UnixStream, request: &Request) -> Result<()> {
    let payload = bincode::serde::encode_to_vec(request, bincode::config::standard())
        .map_err(|err| Error::Invalid(err.to_string()))?;
    let len = (payload.len() as u32).to_be_bytes();
    stream.write_all(&len)?;
    stream.write_all(&payload)?;
    stream.flush()?;
    Ok(())
}

fn read_response(stream: &mut UnixStream) -> Result<Response> {
    let mut len_buf = [0u8; 4];
    stream.read_exact(&mut len_buf)?;
    let len = u32::from_be_bytes(len_buf) as usize;
    let mut buf = vec![0u8; len];
    stream.read_exact(&mut buf)?;
    let (response, _read) =
        bincode::serde::decode_from_slice(&buf, bincode::config::standard())
            .map_err(|err| Error::Invalid(err.to_string()))?;
    Ok(response)
}

fn handle_response(response: Response) -> Result<()> {
    match response {
        Response::Ok => {
            println!("ok");
            Ok(())
        }
        Response::Value(value) => {
            match value {
                Some(bytes) => print_bytes(&bytes),
                None => println!("not found"),
            }
            Ok(())
        }
        Response::Stats { keys } => {
            println!("{keys}");
            Ok(())
        }
        Response::Error(message) => Err(Error::Invalid(message)),
    }
}

fn print_bytes(bytes: &[u8]) {
    if let Ok(text) = std::str::from_utf8(bytes) {
        println!("{text}");
        return;
    }
    let mut hex = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        let _ = write!(hex, "{:02x}", byte);
    }
    println!("{hex}");
}
