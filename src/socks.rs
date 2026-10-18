//! Servidor SOCKS5 mínimo (sin autenticación, solo CONNECT) para los túneles dinámicos `-D`.

use std::io;

use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Reply {
    Succeeded,
    ConnectionRefused,
    CommandNotSupported,
    AddressTypeNotSupported,
}

impl Reply {
    fn code(self) -> u8 {
        match self {
            Reply::Succeeded => 0x00,
            Reply::ConnectionRefused => 0x05,
            Reply::CommandNotSupported => 0x07,
            Reply::AddressTypeNotSupported => 0x08,
        }
    }
}

fn invalid(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message.to_string())
}

/// Negocia la versión y lee la petición CONNECT. Devuelve el destino pedido.
pub async fn handshake<S: AsyncRead + AsyncWrite + Unpin>(stream: &mut S) -> io::Result<(String, u16)> {
    let mut head = [0u8; 2];
    stream.read_exact(&mut head).await?;
    if head[0] != 5 {
        return Err(invalid("solo se admite SOCKS5"));
    }
    let mut methods = vec![0u8; head[1] as usize];
    stream.read_exact(&mut methods).await?;
    if !methods.contains(&0) {
        stream.write_all(&[5, 0xff]).await?;
        return Err(invalid("el cliente exige autenticación"));
    }
    stream.write_all(&[5, 0]).await?;

    let mut request = [0u8; 4];
    stream.read_exact(&mut request).await?;
    if request[0] != 5 {
        return Err(invalid("petición SOCKS5 inválida"));
    }
    if request[1] != 1 {
        reply(stream, Reply::CommandNotSupported).await?;
        return Err(invalid("solo se admite CONNECT"));
    }
    let host = match request[3] {
        1 => {
            let mut octets = [0u8; 4];
            stream.read_exact(&mut octets).await?;
            std::net::Ipv4Addr::from(octets).to_string()
        }
        3 => {
            let mut length = [0u8; 1];
            stream.read_exact(&mut length).await?;
            let mut name = vec![0u8; length[0] as usize];
            stream.read_exact(&mut name).await?;
            String::from_utf8(name).map_err(|_| invalid("nombre de host inválido"))?
        }
        4 => {
            let mut octets = [0u8; 16];
            stream.read_exact(&mut octets).await?;
            std::net::Ipv6Addr::from(octets).to_string()
        }
        _ => {
            reply(stream, Reply::AddressTypeNotSupported).await?;
            return Err(invalid("tipo de dirección no soportado"));
        }
    };
    let mut port = [0u8; 2];
    stream.read_exact(&mut port).await?;
    Ok((host, u16::from_be_bytes(port)))
}

pub async fn reply<S: AsyncWrite + Unpin>(stream: &mut S, reply: Reply) -> io::Result<()> {
    // Dirección de enlace 0.0.0.0:0: el cliente no la necesita.
    stream.write_all(&[5, reply.code(), 0, 1, 0, 0, 0, 0, 0, 0]).await
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn run(client_bytes: Vec<u8>) -> (io::Result<(String, u16)>, Vec<u8>) {
        let (mut client, mut server) = tokio::io::duplex(1024);
        client.write_all(&client_bytes).await.unwrap();
        client.shutdown().await.unwrap();
        let result = handshake(&mut server).await;
        drop(server);
        let mut sent = Vec::new();
        client.read_to_end(&mut sent).await.unwrap();
        (result, sent)
    }

    #[tokio::test]
    async fn connects_to_an_ipv4_address() {
        let (result, sent) = run(vec![5, 1, 0, 5, 1, 0, 1, 10, 0, 0, 7, 0x1f, 0x90]).await;
        assert_eq!(result.unwrap(), ("10.0.0.7".to_string(), 8080));
        assert_eq!(sent, vec![5, 0]);
    }

    #[tokio::test]
    async fn connects_to_a_domain_name() {
        let mut bytes = vec![5, 2, 2, 0, 5, 1, 0, 3, 11];
        bytes.extend_from_slice(b"example.com");
        bytes.extend_from_slice(&443u16.to_be_bytes());
        let (result, _) = run(bytes).await;
        assert_eq!(result.unwrap(), ("example.com".to_string(), 443));
    }

    #[tokio::test]
    async fn connects_to_an_ipv6_address() {
        let mut bytes = vec![5, 1, 0, 5, 1, 0, 4];
        bytes.extend_from_slice(&std::net::Ipv6Addr::LOCALHOST.octets());
        bytes.extend_from_slice(&22u16.to_be_bytes());
        let (result, _) = run(bytes).await;
        assert_eq!(result.unwrap(), ("::1".to_string(), 22));
    }

    #[tokio::test]
    async fn rejects_other_versions_and_unsupported_commands() {
        let (result, _) = run(vec![4, 1, 0, 80]).await;
        assert!(result.is_err());

        let (result, sent) = run(vec![5, 1, 0, 5, 2, 0, 1, 1, 1, 1, 1, 0, 80]).await;
        assert!(result.is_err());
        assert_eq!(sent, vec![5, 0, 5, 7, 0, 1, 0, 0, 0, 0, 0, 0]);
    }

    #[tokio::test]
    async fn refuses_clients_that_demand_authentication() {
        let (result, sent) = run(vec![5, 1, 2]).await;
        assert!(result.is_err());
        assert_eq!(sent, vec![5, 0xff]);
    }
}
