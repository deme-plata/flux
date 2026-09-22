use flux_skyskraber::datacenter::{self,config::Config,planning::{self,Scenario},runtime};
fn main(){if let Err(e)=entry(){eprintln!("datacenter: {e}");std::process::exit(1);}}
fn entry()->datacenter::Result<()> {
    let args:Vec<String>=std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("init") if args.len()==2=>{let c=Config::default();datacenter::storage::private_file(std::path::Path::new(&args[1]),&serde_json::to_vec_pretty(&c)?)?;println!("Created {}",args[1]);}
        Some("plan") if args.len()<=2=>{let s=if args.len()==2{serde_json::from_slice::<Scenario>(&std::fs::read(&args[1])?)?}else{Scenario::default()};println!("{}",serde_json::to_string_pretty(&planning::estimate(&s)?)?);}
        Some("audit") if args.len()==2=>{let c:Config=serde_json::from_slice(&std::fs::read(&args[1])?)?;c.validate()?;println!("{}",serde_json::to_string_pretty(&runtime::audit(&c)?)?);}
        Some("start") if args.len()==2 || (args.len()==4 && args[2]=="--run-seconds")=>{
            let c:Config=serde_json::from_slice(&std::fs::read(&args[1])?)?;c.validate()?;
            let seconds=if args.len()==4{Some(args[3].parse::<u64>()?)}else{None};
            tokio::runtime::Builder::new_multi_thread().worker_threads(2).max_blocking_threads(c.workers).enable_all().build()?.block_on(runtime::run(c,seconds))?;
        }
        _=>return Err("usage: quillon-datacenter init CONFIG | plan [SCENARIO] | audit CONFIG | start CONFIG [--run-seconds N]".into()),
    } Ok(())
}
