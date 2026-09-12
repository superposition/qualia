use std::net::{TcpListener, TcpStream};
use std::time::{Duration, Instant};
use qualia_console::views::brain::connectome::ConnectomeStream;
use qualia_connectome_stream::{SpikeFrame, SpikeWriter};

fn wait_until(mut condition: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(6);
    while !condition() {
        assert!(Instant::now() < deadline, "live reader did not recover within six seconds");
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn accept(listener: &TcpListener) -> TcpStream {
    let mut stream = None;
    wait_until(|| { stream = listener.accept().ok().map(|pair| pair.0); stream.is_some() });
    stream.unwrap()
}

#[test]
fn a_live_source_recovers_from_initial_refusal_and_clean_disconnect() {
    let reservation = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = reservation.local_addr().unwrap();
    drop(reservation);
    let source = ConnectomeStream::open(&format!("tcp://{address}"));
    wait_until(|| source.frame().error.is_some());
    let listener = TcpListener::bind(address).unwrap();
    listener.set_nonblocking(true).unwrap();
    let mut writer = SpikeWriter::new(accept(&listener)).unwrap();
    writer.write_frame(&SpikeFrame::new(10, 1000, vec![3, 7]).unwrap()).unwrap();
    writer.flush().unwrap();
    wait_until(|| source.frame().tick == 10);
    assert_eq!(source.frame().firing, vec![3, 7]);
    let first_advance = source.frame().last_advanced;
    drop(writer);
    wait_until(|| source.frame().error.is_some());
    assert!(!source.frame().finished, "a live disconnect is not recorded EOF");
    let mut writer = SpikeWriter::new(accept(&listener)).unwrap();
    writer.write_frame(&SpikeFrame::new(10, 1000, vec![3, 7]).unwrap()).unwrap();
    writer.flush().unwrap();
    wait_until(|| source.frame().ticks_read == 2);
    assert_eq!(source.frame().last_advanced, first_advance, "replayed cached frame must not refresh source age");
    drop(writer);
    wait_until(|| source.frame().error.is_some());
    let mut writer = SpikeWriter::new(accept(&listener)).unwrap();
    writer.write_frame(&SpikeFrame::new(0, 2000, vec![11]).unwrap()).unwrap();
    writer.flush().unwrap();
    wait_until(|| source.frame().firing == vec![11]);
    assert_eq!(source.frame().tick, 0, "a restarted producer can start at tick zero");
    assert!(source.frame().error.is_none());
    assert!(!source.frame().finished);
    assert!(source.frame().last_advanced > first_advance);
}
