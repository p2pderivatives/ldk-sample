use std::str::SplitWhitespace;

use crate::hex_utils::to_slice;

pub(crate) fn read_id(
	words: &mut SplitWhitespace, err_cmd: &str, err_arg: &str,
) -> Result<[u8; 32], ()> {
	let s = try_read_word(words, err_cmd, err_arg)?;
	let mut res = [0u8; 32];
	match to_slice(&s, &mut res) {
		Err(_) => {
			println!("ERROR: invalid {}.", err_arg);
			Err(())
		}
		Ok(_) => Ok(res),
	}
}

pub(crate) fn try_read_word(
	words: &mut SplitWhitespace, err_cmd: &str, err_arg: &str,
) -> Result<String, ()> {
	match words.next() {
		None => {
			println!("ERROR: {} expects the {} as parameter.", err_cmd, err_arg);
			Err(())
		}
		Some(s) => Ok(s.to_string()),
	}
}
