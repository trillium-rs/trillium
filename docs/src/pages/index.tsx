import React from "react";
import clsx from "clsx";
import Link from "@docusaurus/Link";
import useDocusaurusContext from "@docusaurus/useDocusaurusContext";
import Layout from "@theme/Layout";
import CodeBlock from "@theme/CodeBlock";
import Heading from "@theme/Heading";

import styles from "./index.module.css";

const EXAMPLE = `\
use trillium::{Conn, Handler};
use trillium_logger::Logger;
use trillium_router::Router;

fn app() -> impl Handler {
    (
        Logger::new(),
        Router::new()
            .get("/", "hello world")
            .get("/greet/:name", |conn: Conn| async move {
                let name = conn.param("name").unwrap_or("stranger");
                conn.ok(format!("hello, {name}!"))
            }),
    )
}

fn main() {
    trillium_tokio::run(app());
}`;

type EcosystemItem = {
  name: string;
  description: string;
  href: string;
};

type EcosystemCategory = {
  title: string;
  items: EcosystemItem[];
};

const ecosystem: EcosystemCategory[] = [
  {
    title: "Routing & API",
    items: [
      {
        name: "Router",
        description: "Pattern-based routing with named params and wildcards",
        href: "/guide/handlers/router",
      },
      {
        name: "API",
        description: "Extractor-based handlers with JSON in and out",
        href: "/guide/handlers/api",
      },
    ],
  },
  {
    title: "Observability",
    items: [
      {
        name: "Logger",
        description: "Configurable HTTP request logging",
        href: "/guide/handlers/logger",
      },
      {
        name: "Conn ID",
        description: "Unique identifier per request",
        href: "/guide/handlers/utilities#conn-id",
      },
      {
        name: "OpenTelemetry",
        description: "Tracing and metrics",
        href: "https://docs.rs/trillium-opentelemetry",
      },
    ],
  },
  {
    title: "Auth, Cookies & Sessions",
    items: [
      {
        name: "Basic Auth",
        description: "HTTP Basic Authentication",
        href: "/guide/handlers/utilities#basic-auth",
      },
      {
        name: "Cookies",
        description: "Parse and set cookies",
        href: "/guide/handlers/cookies",
      },
      {
        name: "Sessions",
        description: "Server-side sessions with pluggable stores",
        href: "/guide/handlers/sessions",
      },
    ],
  },
  {
    title: "Content",
    items: [
      {
        name: "Static Files",
        description: "Serve files from disk",
        href: "/guide/handlers/static",
      },
      {
        name: "Static Compiled",
        description: "Embed assets in the binary at compile time",
        href: "/guide/handlers/static",
      },
      {
        name: "Templates",
        description: "Askama, Tera, and Handlebars",
        href: "/guide/handlers/templates",
      },
      {
        name: "Compression",
        description: "gzip, brotli, and zstd via Accept-Encoding",
        href: "/guide/handlers/utilities#compression",
      },
      {
        name: "Caching Headers",
        description: "ETag and Last-Modified with 304 support",
        href: "/guide/handlers/utilities#caching-headers",
      },
    ],
  },
  {
    title: "Real-time",
    items: [
      {
        name: "Server-Sent Events",
        description: "Lightweight server-to-client event streams",
        href: "/guide/handlers/sse",
      },
      {
        name: "WebSockets",
        description: "Full-duplex connections with original request context",
        href: "/guide/handlers/websockets",
      },
      {
        name: "Channels",
        description: "Phoenix-style topic pub/sub over WebSocket",
        href: "/guide/handlers/channels",
      },
      {
        name: "WebTransport",
        description: "Multiplexed streams and datagrams over HTTP/3",
        href: "/guide/handlers/webtransport",
      },
    ],
  },
  {
    title: "Client & Proxy",
    items: [
      {
        name: "HTTP Client",
        description: "Async HTTP/1.1 and HTTP/3 client with connection pooling",
        href: "/guide/handlers/http_client",
      },
      {
        name: "Reverse Proxy",
        description: "Forward requests to upstream servers",
        href: "/guide/handlers/proxy",
      },
      {
        name: "HTML Rewriter",
        description: "Inject or modify content as it streams through a proxy",
        href: "https://docs.rs/trillium-html-rewriter",
      },
    ],
  },
  {
    title: "TLS",
    items: [
      {
        name: "Rustls",
        description: "TLS via rustls",
        href: "/guide/overview/runtimes#rustls",
      },
      {
        name: "Native TLS",
        description: "TLS via native-tls",
        href: "/guide/overview/runtimes#native-tls",
      },
      {
        name: "ACME",
        description: "Automatic certificate provisioning via Let's Encrypt",
        href: "https://docs.rs/trillium-acme",
      },
      {
        name: "Quinn",
        description: "HTTP/3 over QUIC",
        href: "/guide/overview/runtimes#http3-and-quic",
      },
    ],
  },
  {
    title: "Runtimes",
    items: [
      {
        name: "Tokio",
        description: "Tokio runtime adapter",
        href: "/guide/overview/runtimes#runtime-adapters",
      },
      {
        name: "Smol",
        description: "Smol runtime adapter — lightweight and fast",
        href: "/guide/overview/runtimes#runtime-adapters",
      },
      {
        name: "async-std",
        description: "async-std runtime adapter",
        href: "/guide/overview/runtimes#runtime-adapters",
      },
      {
        name: "AWS Lambda",
        description: "Lambda adapter — TLS handled by the load balancer",
        href: "/guide/overview/runtimes#runtime-adapters",
      },
    ],
  },
  {
    title: "Testing",
    items: [
      {
        name: "Testing",
        description:
          "Full HTTP stack exercised without binding a port — fluent assertions, any handler in isolation",
        href: "/guide/testing",
      },
    ],
  },
];

type FeatureItem = {
  title: string;
  description: React.ReactNode;
};

const features: FeatureItem[] = [
  {
    title: "Thoughtful Abstraction",
    description: (
      <>
        A <code>Conn</code> carries request, response, and state as a single
        object through your entire stack. Every component — logger, router, auth
        gate, endpoint — is a <code>Handler</code> that transforms it. One
        abstraction, no special cases, no impedance between layers.
      </>
    ),
  },
  {
    title: "HTTP/3",
    description:
      "HTTP/3 over QUIC alongside HTTP/1.x — add a crate and two lines of config, and existing handlers run unchanged across protocols. WebTransport, for bidirectional browser communication over HTTP/3, is included.",
  },
  {
    title: "Runtime and TLS Independent",
    description:
      "Choose tokio, smol, async-std, or AWS Lambda — swap with one import. TLS (rustls or native-tls) is equally orthogonal. Infrastructure choices live at the application boundary, invisible to handler code.",
  },
  {
    title: "Opt-in Composable Ecosystem",
    description:
      "Router, API layer, sessions, WebSockets, channels, SSE, static files, compression, templates, reverse proxy — all independent crates that compose the same way. Add what you need; nothing else compiles.",
  },
  {
    title: "Matching HTTP Client",
    description: (
      <>
        A full async HTTP client: runtime-agnostic, TLS-independent, and
        protocol-forward. Upgrades to HTTP/3 automatically via Alt-Svc, supports
        WebSocket upgrades, and shares your server's runtime without friction.
      </>
    ),
  },
  {
    title: "Integration Testing Framework",
    description: (
      <>
        <code>trillium-testing</code> exercises the full HTTP stack without
        binding a port. Test any handler — a single component or the full
        application — with a fluent assertion API.
      </>
    ),
  },
];

export default function Home(): React.JSX.Element {
  const { siteConfig } = useDocusaurusContext();
  return (
    <Layout title={siteConfig.title} description={siteConfig.tagline}>
      <header className={clsx("hero", styles.hero)}>
        <div className="container">
          <div className={styles.heroInner}>
            <div className={styles.heroText}>
              <Heading as="h1" className={styles.heroTitle}>
                trillium
              </Heading>
              <p className={styles.heroSubtitle}>{siteConfig.tagline}</p>
              <p className={styles.heroDescription}>
                Trillium is a modular async Rust web toolkit. The components you
                choose and how you arrange them are your configuration. Add what
                you need, publish what you build, swap what you outgrow.
              </p>
              <div className={styles.buttons}>
                <Link
                  className="button button--primary button--lg"
                  to="/guide/welcome"
                >
                  Get started
                </Link>
                <Link
                  className="button button--secondary button--lg"
                  href="https://github.com/trillium-rs/trillium"
                >
                  <svg
                    viewBox="0 0 16 16"
                    aria-hidden="true"
                    style={{
                      width: "1em",
                      height: "1em",
                      verticalAlign: "-0.125em",
                      marginRight: "0.4em",
                      fill: "currentColor",
                    }}
                  >
                    <path d="M8 0C3.58 0 0 3.58 0 8c0 3.54 2.29 6.53 5.47 7.59.4.07.55-.17.55-.38 0-.19-.01-.82-.01-1.49-2.01.37-2.53-.49-2.69-.94-.09-.23-.48-.94-.82-1.13-.28-.15-.68-.52-.01-.53.63-.01 1.08.58 1.23.82.72 1.21 1.87.87 2.33.66.07-.52.28-.87.51-1.07-1.78-.2-3.64-.89-3.64-3.95 0-.87.31-1.59.82-2.15-.08-.2-.36-1.02.08-2.12 0 0 .67-.21 2.2.82.64-.18 1.32-.27 2-.27.68 0 1.36.09 2 .27 1.53-1.04 2.2-.82 2.2-.82.44 1.1.16 1.92.08 2.12.51.56.82 1.27.82 2.15 0 3.07-1.87 3.75-3.65 3.95.29.25.54.73.54 1.48 0 1.07-.01 1.93-.01 2.2 0 .21.15.46.55.38A8.013 8.013 0 0016 8c0-4.42-3.58-8-8-8z" />
                  </svg>
                  GitHub
                </Link>
              </div>
            </div>
            <div className={styles.heroCode}>
              <CodeBlock language="rust">{EXAMPLE}</CodeBlock>
            </div>
          </div>
        </div>
      </header>
      <main>
        <section className={styles.features}>
          <div className="container">
            <div className="row">
              {features.map(({ title, description }) => (
                <div key={title} className={clsx("col col--4")}>
                  <div className={styles.feature}>
                    <Heading as="h3">{title}</Heading>
                    <p>{description}</p>
                  </div>
                </div>
              ))}
            </div>
          </div>
        </section>
        <section className={styles.ecosystem}>
          <div className="container">
            <Heading as="h2" className={styles.ecosystemHeading}>
              Ecosystem
            </Heading>
            <div className={styles.ecosystemGrid}>
              {ecosystem.map(({ title, items }) => (
                <div key={title} className={styles.ecosystemCategory}>
                  <h3 className={styles.ecosystemCategoryTitle}>{title}</h3>
                  <ul className={styles.ecosystemList}>
                    {items.map(({ name, description, href }) => (
                      <li key={name}>
                        <Link href={href} className={styles.ecosystemItemName}>
                          {name}
                        </Link>
                        <span className={styles.ecosystemItemDesc}>
                          {" "}
                          — {description}
                        </span>
                      </li>
                    ))}
                  </ul>
                </div>
              ))}
            </div>
            <p className={styles.ecosystemCta}>
              Any <code>Handler</code> composes.{" "}
              <Link to="/guide/library_patterns">
                Build and publish your own.
              </Link>
            </p>
          </div>
        </section>

        <section className={styles.statement}>
          <div className="container">
            <div className={styles.statementInner}>
              <Heading as="h2" className={styles.statementHeading}>
                Trillium 1.0
              </Heading>
              <p className={styles.statementText}>
                Trillium takes semver seriously. Breaking changes are deliberate
                and infrequent. The release of 1.0 reflects a commitment to the
                current API shape and a considered approach to how and when it
                evolves from here. There's an extensive roadmap for further
                trillium features, but breaking changes should be extremely
                rare.
              </p>
              <p className={styles.statementText}>
                Trillium is actively developed and available for commercial
                support and consulting. If your organization is building on
                trillium and would benefit from professional support,
                architecture review, or custom development, I'd love to hear
                from you.
              </p>
              <div className={styles.statementButtons}>
                <Link
                  className="button button--primary button--lg"
                  to="/guide/architecture"
                >
                  Learn more
                </Link>
                <Link
                  className="button button--secondary button--lg"
                  href="https://jbr.me"
                >
                  Get in touch
                </Link>
              </div>
            </div>
          </div>
        </section>
      </main>
    </Layout>
  );
}
