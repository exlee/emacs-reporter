const MOTD_URL: &str = "https://xlii.space/emacs/motd.txt";
const BORDER: &str = "────────────────────────────────";

pub fn fetch_and_print() {
    let (tx, rx) = std::sync::mpsc::channel();

    std::thread::spawn(move || {
        let result = ureq::get(MOTD_URL).call();
        let _ = tx.send(result);
    });

    let Ok(Ok(response)) = rx.recv_timeout(std::time::Duration::from_millis(500)) else {
        return;
    };

    let Ok(body) = response.into_body().read_to_string() else {
        return;
    };

    let body = body.trim();
    if !body.is_empty() {
        println!("{BORDER}\n{body}\n{BORDER}");
    }
}
