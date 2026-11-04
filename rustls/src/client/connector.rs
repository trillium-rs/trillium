use super::*;

impl<C: Connector> RustlsConfig<C> {
    fn tls_connector(&self, destination: &Destination) -> TlsConnector {
        // A per-connection ALPN override replaces the config's default; absent one, the shared
        // config is used as-is. Only clone the (otherwise shared) config when an override is
        // present.
        let rustls_config = if let Some(alpn) = destination.alpn() {
            let mut config = (*self.rustls_config.0).clone();
            config.alpn_protocols = alpn.iter().map(|p| p.to_vec()).collect();
            Arc::new(config)
        } else {
            Arc::clone(&self.rustls_config.0)
        };
        rustls_config.into()
    }
}

fn domain_server_name(destination: &Destination) -> Result<Option<ServerName<'static>>> {
    destination
        .host()
        .map(|domain| {
            ServerName::try_from(domain.to_owned()).map_err(|e| {
                Error::new(
                    ErrorKind::InvalidInput,
                    format!("invalid server name {domain:?}: {e}"),
                )
            })
        })
        .transpose()
}

fn destination_server_name(destination: &Destination) -> Result<ServerName<'static>> {
    if let Some(server_name) = domain_server_name(destination)? {
        Ok(server_name)
    } else {
        let address = destination.addrs().first().ok_or_else(|| {
            Error::new(
                ErrorKind::InvalidInput,
                "bare-ip destination has no socket address",
            )
        })?;
        Ok(ServerName::IpAddress(address.ip().into()))
    }
}

impl<C: Connector> Connector for RustlsConfig<C> {
    type Runtime = C::Runtime;
    type Transport = RustlsClientTransport<C::Transport>;
    type Udp = C::Udp;

    async fn connect(&self, url: &Url) -> Result<Self::Transport> {
        self.connect_to(Destination::from_url(url)?).await
    }

    async fn connect_to(&self, destination: Destination) -> Result<Self::Transport> {
        if !destination.secure() {
            return self
                .tcp_config
                .connect_to(destination)
                .await
                .map(Into::into);
        }

        let connector = self.tls_connector(&destination);

        // A domain destination's certificate identity (a DNS `ServerName`, sent via SNI) is fixed
        // before the dial, so pre-resolved addresses can't influence validation. A host-less
        // (bare-IP) destination has no SNI and validates against the address actually connected to,
        // so its `IpAddress` server name is derived from the dialed stream below.
        let domain_server_name = domain_server_name(&destination)?;

        let stream = self
            .tcp_config
            .connect_to(destination.with_secure(false))
            .await?;

        let server_name = match domain_server_name {
            Some(server_name) => server_name,
            None => {
                let ip = stream
                    .peer_addr()?
                    .ok_or_else(|| Error::other("no peer address for bare-ip destination"))?
                    .ip();
                ServerName::IpAddress(ip.into())
            }
        };

        connector
            .connect(server_name, stream)
            .await
            .map_err(|e| Error::other(e.to_string()))
            .map(Into::into)
    }

    async fn connect_over(
        &self,
        transport: Box<dyn Transport>,
        destination: Destination,
    ) -> Result<Box<dyn Transport>> {
        if !destination.secure() {
            return self.tcp_config.connect_over(transport, destination).await;
        }

        let server_name = destination_server_name(&destination)?;
        let connector = self.tls_connector(&destination);
        let stream = connector
            .connect(server_name, transport)
            .await
            .map_err(|e| Error::other(e.to_string()))?;
        Ok(Box::new(RustlsClientTransport::<Box<dyn Transport>>::from(
            stream,
        )))
    }

    fn runtime(&self) -> Self::Runtime {
        self.tcp_config.runtime()
    }

    async fn resolve(&self, host: &str, port: u16) -> Result<Vec<SocketAddr>> {
        self.tcp_config.resolve(host, port).await
    }
}
