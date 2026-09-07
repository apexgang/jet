//! File-controlled native peer: no timing assumptions decide turn completion.
use serde_json::{Value, json};
use std::{io::Write, path::Path, time::Duration};

pub fn harness() {
	let mut id = "initial".to_string();
	loop {
		std::fs::write("current-turn", &id).unwrap();
		let interrupted = format!("interrupted-{id}");
		while Path::new("queue").exists()
			&& !Path::new(&format!("continue-{id}")).exists()
			&& !Path::new(&interrupted).exists()
		{
			std::thread::sleep(Duration::from_millis(10));
		}
		if !Path::new("queue").exists() {
			return;
		}
		// A cancelled turn ends without a completion; the Harness itself
		// stays alive and waits for the next input.
		if Path::new(&interrupted).exists() {
			println!("{}", json!({"interrupted_turn":id,"text":"Cancelled"}));
		} else {
			let completion =
				if std::fs::read_to_string(format!("continue-{id}")).unwrap()
					== "wrong"
				{
					uuid::Uuid::nil().to_string()
				} else {
					id.clone()
				};
			println!(
				"{}",
				json!({"turn_id":completion,"text":"Turn finished"})
			);
		}
		std::io::stdout().flush().unwrap();
		let request = loop {
			if !Path::new("queue").exists() {
				return;
			}
			if let Ok(bytes) = std::fs::read("turn-input") {
				break serde_json::from_slice::<Value>(&bytes).unwrap();
			}
			std::thread::sleep(Duration::from_millis(10));
		};
		std::fs::remove_file("turn-input").unwrap();
		id = request["id"].as_str().unwrap().into();
		let mut log = std::fs::OpenOptions::new()
			.create(true)
			.append(true)
			.open("delivered-turns")
			.unwrap();
		writeln!(log, "{request}").unwrap();
	}
}
