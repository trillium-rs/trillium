use structopt::StructOpt;

#[derive(StructOpt, Debug)]
pub enum Cli {
    Static(crate::StaticCli),
    #[cfg(unix)]
    DevServer(crate::DevServer),
    Client(crate::ClientCli),
}

impl Cli {
    pub fn run(self) {
        use Cli::*;
        match self {
            Static(s) => s.run(),
            #[cfg(unix)]
            DevServer(d) => d.run(),
            Client(c) => c.run(),
        }
    }
}
