import React from 'react';
import clsx from 'clsx';
import Link from '@docusaurus/Link';
import useDocusaurusContext from '@docusaurus/useDocusaurusContext';
import Layout from '@theme/Layout';
import CodeBlock from '@theme/CodeBlock';
import Heading from '@theme/Heading';

import styles from './index.module.css';

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

type FeatureItem = {
  title: string;
  description: string;
};

const features: FeatureItem[] = [
  {
    title: 'No Middleware Layer',
    description:
      'Handlers and middleware are the same abstraction. Compose them in a tuple — the pipeline runs left to right, stopping at the first handler that halts. There is no separate middleware API to learn.',
  },
  {
    title: 'Runtime-Agnostic',
    description:
      'Switch between Tokio, Smol, and async-std with a single import. Your application code and all handler libraries work identically across all three runtimes.',
  },
  {
    title: 'HTTP/3 Ready',
    description:
      'First-class HTTP/3 over QUIC via trillium-quinn. Add one line to your server config and existing handlers run over HTTP/3 without any modification.',
  },
];

export default function Home(): React.JSX.Element {
  const {siteConfig} = useDocusaurusContext();
  return (
    <Layout title={siteConfig.title} description={siteConfig.tagline}>
      <header className={clsx('hero', styles.hero)}>
        <div className="container">
          <div className={styles.heroInner}>
            <div className={styles.heroText}>
              <Heading as="h1" className={styles.heroTitle}>
                trillium
              </Heading>
              <p className={styles.heroSubtitle}>{siteConfig.tagline}</p>
              <div className={styles.buttons}>
                <Link
                  className="button button--primary button--lg"
                  to="/guide/welcome">
                  Get started
                </Link>
                <Link
                  className="button button--secondary button--lg"
                  href="https://github.com/trillium-rs/trillium">
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
              {features.map(({title, description}) => (
                <div key={title} className={clsx('col col--4')}>
                  <div className={styles.feature}>
                    <Heading as="h3">{title}</Heading>
                    <p>{description}</p>
                  </div>
                </div>
              ))}
            </div>
          </div>
        </section>
      </main>
    </Layout>
  );
}
