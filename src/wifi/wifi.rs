use blocking_network_stack::Stack;
use esp_alloc::HeapStats;
use esp_hal::{peripherals::WIFI, rng::Rng, time};
use esp_println::println;
use esp_radio::wifi::{AccessPointConfig, ModeConfig, WifiController};
use smoltcp::iface::{SocketSet, SocketStorage};

extern crate alloc;
use alloc::boxed::Box;

pub fn init_wifi_stack<'a>(
    wifi: WIFI<'a>,
) -> (
    Stack<'a, esp_radio::wifi::WifiDevice<'a>>,
    WifiController<'a>,
) {
    let esp_radio_ctrl = Box::new(esp_radio::init().unwrap());
    let box_ctrl = Box::leak(esp_radio_ctrl);
    // 1. Init wifi
    // interrupts must be active when we call this new
    let (mut controller, interfaces) =
        esp_radio::wifi::new(box_ctrl, wifi, Default::default()).unwrap();

    // 2. obtain devices and interfaces that will be used to build smoltcp::stack
    let mut ap_device = interfaces.ap;
    let ap_interface = create_interface(&mut ap_device);

    // 3. source of randomness and time. (not sure)
    let rng = Rng::new();
    let now = || time::Instant::now().duration_since_epoch().as_millis();

    // 4. build AP
    // AP stack setup
    let ap_socket_set_entries: Box<[SocketStorage; 3]> = Box::new(Default::default());
    let boxed_set = Box::leak(ap_socket_set_entries);
    let ap_socket_set = SocketSet::new(&mut boxed_set[..]);
    let mut ap_stack: Stack<'_, esp_radio::wifi::WifiDevice<'_>> =
        Stack::new(ap_interface, ap_device, ap_socket_set, now, rng.random());

    // 5. Crate access point
    let client_config =
        ModeConfig::AccessPoint(AccessPointConfig::default().with_ssid("esp-radio".into()));

    // 6. Configure WiFi controler at `our` chip
    // with new AccessPoint connections
    let res = controller.set_config(&client_config);
    println!("wifi_set_configuration returned {:?}", res);

    controller.start().unwrap();
    println!("is wifi started: {:?}", controller.is_started());

    println!("{:?}", controller.capabilities());

    // 7. Init our AP stack; later we will connect to it
    // the goal of smoltcp is not just to provide a simple interface for writing applications
    // but also to be a toolbox of networking primitives, so every layer is fully exposed and documented
    // so with stack we could control `everything` we might need
    ap_stack
        .set_iface_configuration(&blocking_network_stack::ipv4::Configuration::Client(
            blocking_network_stack::ipv4::ClientConfiguration::Fixed(
                blocking_network_stack::ipv4::ClientSettings {
                    ip: blocking_network_stack::ipv4::Ipv4Addr::from(parse_ip("192.168.2.1")),
                    subnet: blocking_network_stack::ipv4::Subnet {
                        gateway: blocking_network_stack::ipv4::Ipv4Addr::from(parse_ip(
                            "192.168.2.1",
                        )),
                        mask: blocking_network_stack::ipv4::Mask(24),
                    },
                    dns: None,
                    secondary_dns: None,
                },
            ),
        ))
        .unwrap();

    let stats: HeapStats = esp_alloc::HEAP.stats();
    println!("{}", stats);

    (ap_stack, controller)
}

// some smoltcp boilerplate
fn timestamp() -> smoltcp::time::Instant {
    smoltcp::time::Instant::from_micros(
        esp_hal::time::Instant::now()
            .duration_since_epoch()
            .as_micros() as i64,
    )
}

fn parse_ip(ip: &str) -> [u8; 4] {
    let mut result = [0u8; 4];
    for (idx, octet) in ip.split(".").into_iter().enumerate() {
        result[idx] = u8::from_str_radix(octet, 10).unwrap();
    }
    result
}

pub fn create_interface(device: &mut esp_radio::wifi::WifiDevice) -> smoltcp::iface::Interface {
    // users could create multiple instances but since they only have one WifiDevice
    // they probably can't do anything bad with that
    smoltcp::iface::Interface::new(
        smoltcp::iface::Config::new(smoltcp::wire::HardwareAddress::Ethernet(
            smoltcp::wire::EthernetAddress::from_bytes(&device.mac_address()),
        )),
        device,
        timestamp(),
    )
}
