use clap::{Args, Parser, Subcommand};

///TODO: Program description
#[derive(Parser, Debug)]
#[command(version)]
struct Cli {
    #[arg(short)] //TODO default value
    cwd: Option<String>,

    #[command(subcommand)]
    command: Commands,

}

#[derive(Subcommand, Debug)]
enum Commands {
    Init(Init),
    Config,
    Refilter,
    Fetch(Fetch),
    Push,
}

#[derive(Args, Debug)]
struct Init {
    repository: String,
    directory: String,
}

#[derive(Args, Debug)]
struct Fetch {
    #[arg(long)]
    skip_filter: bool,
    remote: String,
    #[arg(id="ref")]
    reference: String,
}

fn fetch(args: Cli) {
    println!("Fetch!")
}

fn main() {
    let args = Cli::parse();
    println!("{:?}", args);

    match args.command {
        Commands::Init(_) => {}
        Commands::Config => {}
        Commands::Refilter => {}
        Commands::Fetch(_) => {fetch(args)}
        Commands::Push => {}
    }

}
