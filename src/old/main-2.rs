#![no_std]
#![no_main]


use core::fmt::Write;


use esp_hal::{
    clock::ClockControl, peripherals::Peripherals, prelude::*, systimer::SystemTimer, Rng, IO
};
use esp_backtrace as _;
use esp_wifi::{current_millis, initialize, wifi::{utils::create_network_interface, AuthMethod, ClientConfiguration, Configuration, WifiStaDevice}, wifi_interface::{IoError, WifiStack}, EspWifiInitFor};
use smoltcp::{iface::SocketStorage, socket::udp::{RecvError, SendError}, wire::IpAddress};


use usb_writer::UsbWriterBuffered;



mod usb_writer;



enum UdpCommand {
    UsbFulsh,
    LedFaster,
    LedSlower,
}



// const SSID: &str = "Who am I?";
// const PASSWORD: &str = "iamachampion";
const SSID: &str = "jednorozec";
const PASSWORD: &str = "bezpecneheslo";



#[entry]
fn main() -> ! {
    /* init - basics */
    let peripherals = Peripherals::take();

    let system = peripherals.SYSTEM.split();
    let clocks = ClockControl::max(system.clock_control).freeze();

    let io = IO::new(peripherals.GPIO, peripherals.IO_MUX);
    let systimer = SystemTimer::new(peripherals.SYSTIMER);
    let rng = Rng::new(peripherals.RNG);

    /* init - led */
    let mut led = io.pins.gpio20.into_push_pull_output();
    led.set_low().unwrap();

    let led_ts = [
        SystemTimer::TICKS_PER_SECOND / 16,
        SystemTimer::TICKS_PER_SECOND / 8,
        SystemTimer::TICKS_PER_SECOND / 4,
        SystemTimer::TICKS_PER_SECOND / 2,
        SystemTimer::TICKS_PER_SECOND,
        SystemTimer::TICKS_PER_SECOND * 2
    ];

    let mut led_t_index = 3usize;
    let mut led_next_toggle = SystemTimer::now() + led_ts[led_t_index];

    /* init - usb */
    let mut usb_writer = UsbWriterBuffered::<1024>::default().with_usb(peripherals.USB_DEVICE);

    /* init - wifi */
    let init = initialize(EspWifiInitFor::Wifi, systimer.alarm0, rng, system.radio_clock_control, &clocks).unwrap();

    let mut sockets = <[SocketStorage; 3]>::default();
    let (iface, device, mut controller, sockets) = create_network_interface(&init, peripherals.WIFI, WifiStaDevice, &mut sockets).unwrap();

    let client_config = Configuration::Client(ClientConfiguration {
        ssid: SSID.try_into().unwrap(),
        password: PASSWORD.try_into().unwrap(),
        auth_method: AuthMethod::WPAWPA2Personal,
        // bssid: Some([48, 181, 194, 185, 171, 137]),
        // channel: Some(2),
        bssid: Some([236, 67, 246, 119, 162, 116]),
        channel: Some(5),
        ..Default::default()
    });
    let res = controller.set_configuration(&client_config);
    writeln!(usb_writer.twti(), "controller.set_configuration() : {:?}", res).unwrap();

    controller.start().unwrap();

    let aps = controller.scan_n::<20>();
    match aps {
        Ok((aps, _)) => {
            writeln!(usb_writer.twti(), "controller.scan_n() : Ok(...)").unwrap();
            for ap in aps {
                usb_writer.update_blocking();
                writeln!(usb_writer.twti(), "{:?}", ap).unwrap();
            }
        }
        Err(_) => {
            writeln!(usb_writer.twti(), "controller.scan_n() : {:?}", aps).unwrap();
        }
    }

    usb_writer.update_blocking();

    writeln!(usb_writer.twti(), "controller.get_capabilities() : {:?}", controller.get_capabilities()).unwrap();
    writeln!(usb_writer.twti(), "controller.connect() : {:?}", controller.connect()).unwrap();

    writeln!(usb_writer.twti(), "waiting until connected").unwrap();

    usb_writer.update_blocking();

    loop {
        usb_writer.update_without_blocking();

        let is_connected = controller.is_connected();
        match is_connected {
            Ok(is_connected) => {
                if is_connected {
                    break;
                }
            }
            Err(_) => {
                writeln!(usb_writer.twti(), "controller.is_connected() : {:?}", is_connected).unwrap();
                usb_writer.update_blocking_empty();
                panic!();
            }
        }
    }

    writeln!(usb_writer.twti(), "connected").unwrap();

    let wifi_stack = WifiStack::new(iface, device, sockets, current_millis);

    writeln!(usb_writer.twti(), "waiting for ip address").unwrap();

    usb_writer.update_blocking();

    loop {
        usb_writer.update_without_blocking();

        wifi_stack.work();
        if wifi_stack.is_iface_up() {
            break;
        }
    }

    writeln!(usb_writer.twti(), "wifi_stack.get_ip_info() : {:?}", wifi_stack.get_ip_info()).unwrap();

    /* init - udp socket */
    let mut tx_buffer = [0u8; 4096];
    let mut rx_buffer = [0u8; 4096];
    let mut tx_meta_buffer = [smoltcp::socket::udp::PacketMetadata::EMPTY; 10];
    let mut rx_meta_buffer = [smoltcp::socket::udp::PacketMetadata::EMPTY; 10];

    let mut socket = wifi_stack.get_udp_socket(&mut rx_meta_buffer, &mut rx_buffer, &mut tx_meta_buffer, &mut tx_buffer);

    socket.bind(9123).unwrap();

    let mut rx_packet_buffer = [0u8; 1024];
    let mut tx_packet_buffer = [0u8; 1024];

    let default_response = "esp prijalo paket - ";
    let tx_packet_buffer_start = default_response.len();

    tx_packet_buffer[..tx_packet_buffer_start].copy_from_slice(default_response.as_bytes());

    /* loop */
    loop {
        /* update - usb (blocking with timeout, non trivial time) */
        usb_writer.update_blocking();

        /* update - wifi (blocking, non trivial time ???) */
        wifi_stack.work();

        /* update - udp socket */
        let mut command: Option<UdpCommand> = None;

        let recieve_res = socket.receive(&mut rx_packet_buffer);

        match recieve_res {
            Ok((len, ip, port)) => {
                writeln!(usb_writer.twni(), "  ==  recieved new packet ==").unwrap();
                writeln!(usb_writer.twni(), "ip : {}", ip).unwrap();
                writeln!(usb_writer.twni(), "port : {}", port).unwrap();

                let data = &rx_packet_buffer[..len];

                match core::str::from_utf8(data) {
                    Ok(data_str) => {
                        writeln!(usb_writer.twni(), "{}", data_str).unwrap();

                        match data_str {
                            "flush usb" => { command = Some(UdpCommand::UsbFulsh); },
                            "led faster" => { command = Some(UdpCommand::LedFaster); },
                            "led slower" => { command = Some(UdpCommand::LedSlower); },
                            _ => {}
                        }

                        let mut tx_packet_buffer_index = tx_packet_buffer_start;

                        for c in data_str.chars().rev() {
                            tx_packet_buffer_index += c.encode_utf8(&mut tx_packet_buffer[tx_packet_buffer_index..]).len(); // [todo] panics when buffer is too small, prevent panic by checking buffer size
                        }

                        // let send_res = socket.send(IpAddress::v4(192, 168, 0, 108), 9125, &tx_packet_buffer[..tx_packet_buffer_index]);
                        let send_res = socket.send(IpAddress::v4(192, 168, 1, 4), 9125, &tx_packet_buffer[..tx_packet_buffer_index]);

                        match send_res {
                            Ok(()) => {},
                            Err(IoError::UdpSendError(SendError::BufferFull)) => {
                                writeln!(usb_writer.twni(), "socket.receive() info : tx buffer is full").unwrap();
                            },
                            Err(IoError::UdpSendError(SendError::Unaddressable)) => {
                                writeln!(usb_writer.twni(), "socket.receive() info : invalid address").unwrap();
                            },
                            Err(e) => {
                                writeln!(usb_writer.twti(), "socket.send() failed : {:?}", e).unwrap();
                                usb_writer.update_blocking_empty();
                                panic!();
                            },
                        }
                    },
                    Err(_) => {
                        writeln!(usb_writer.twni(), "socket.receive() info : recieved data is not vaild utf-8").unwrap();
                    },
                }
            },
            Err(IoError::UdpRecvError(RecvError::Exhausted)) => {},
            Err(IoError::UdpRecvError(RecvError::Truncated)) => {
                writeln!(usb_writer.twni(), "socket.receive() info : packet truncated error").unwrap();
            },
            Err(e) => {
                writeln!(usb_writer.twti(), "socket.receive() failed : {:?}", e).unwrap();
                usb_writer.update_blocking_empty();
                panic!();
            },
        }

        if let Some(command) = command {
            match command {
                UdpCommand::UsbFulsh => {
                    writeln!(usb_writer.twti(), "force flushing whole usb buffer").unwrap();
                    usb_writer.update_blocking_empty();
                },
                UdpCommand::LedFaster => {
                    if led_t_index > 0 {
                        led_t_index -= 1;
                    }
                },
                UdpCommand::LedSlower => {
                    if led_t_index < led_ts.len() - 1 {
                        led_t_index += 1;
                    }
                },
            }
        }

        /* update - led */
        // let now = SystemTimer::now();

        // if led_next_toggle <= now {
        //     led.toggle().unwrap();
        //     led_next_toggle = now + led_ts[led_t_index];
        // }

        led.set_state(usb_writer.buffer_empty().into()).unwrap();

        // if usb_writer.buffer_empty() {

        // }
    }
}