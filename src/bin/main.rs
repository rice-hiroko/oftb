extern crate failure;
#[macro_use]
extern crate log;
extern crate oftb;
extern crate stderrlog;
#[macro_use]
extern crate structopt;

mod options;

use failure::Error;
use oftb::parse_file;
use oftb::ast::Module;
use structopt::StructOpt;

use options::Options;

fn main() {
    let options = Options::from_args();
    options.start_logger();

    if let Err(err) = run(options) {
        error!("{}", err)
    }
}

fn run(options: Options) -> Result<(), Error> {
    let literals = parse_file(options.path)?;
    println!("Literals:");
    for l in &literals {
        println!("{}", l);
    }

    let ast = Module::from_values(literals)?;
    println!("{:?}", ast);

    Ok(())
}
