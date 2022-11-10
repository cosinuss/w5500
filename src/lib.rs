#![feature(async_closure)]
#![no_std]
#![feature(type_alias_impl_trait)]
#![feature(async_fn_in_trait)]
#![allow(incomplete_features)]
#![allow(clippy::unusual_byte_groupings)]

use core::fmt::Debug;
use core::future::Future;

use byteorder::BigEndian;
use byteorder::ByteOrder;
use core::convert::TryFrom;
use core::str::FromStr;
use core::task::Poll;
use embedded_hal::digital::OutputPin;
use embedded_hal_async::spi::SpiBus;
use futures::TryFutureExt;

const COMMAND_READ: u8 = 0x00 << 2;
const COMMAND_WRITE: u8 = 0x01 << 2;

const VARIABLE_DATA_LENGTH: u8 = 0b_00;

pub async fn yield_now() {
    YieldNow(false).await
}

struct YieldNow(bool);

impl Future for YieldNow {
    type Output = ();

    fn poll(
        mut self: core::pin::Pin<&mut Self>,
        cx: &mut core::task::Context<'_>,
    ) -> Poll<Self::Output> {
        if !self.0 {
            self.0 = true;
            cx.waker().wake_by_ref();
            Poll::Pending
        } else {
            Poll::Ready(())
        }
    }
}

#[derive(Copy, Clone, PartialOrd, PartialEq, Default, Debug)]
pub struct IpAddress {
    pub address: [u8; 4],
}

impl IpAddress {
    pub fn new(a0: u8, a1: u8, a2: u8, a3: u8) -> IpAddress {
        IpAddress {
            address: [a0, a1, a2, a3],
        }
    }
}

impl TryFrom<&str> for IpAddress {
    type Error = ();
    fn try_from(string: &str) -> Result<IpAddress, Self::Error> {
        let mut address = [0u8; 4];
        for (i, part) in string.split('.').enumerate() {
            if i > 3 {
                break;
            }
            address[i] = u8::from_str(part).map_err(|_| ())?;
        }
        Ok(IpAddress { address })
    }
}

impl ::core::fmt::Display for IpAddress {
    fn fmt(&self, f: &mut ::core::fmt::Formatter) -> ::core::fmt::Result {
        write!(
            f,
            "{}.{}.{}.{}",
            self.address[0], self.address[1], self.address[2], self.address[3],
        )
    }
}

#[derive(Copy, Clone, PartialOrd, PartialEq, Default, Debug)]
pub struct MacAddress {
    pub address: [u8; 6],
}

impl MacAddress {
    pub fn new(a0: u8, a1: u8, a2: u8, a3: u8, a4: u8, a5: u8) -> MacAddress {
        MacAddress {
            address: [a0, a1, a2, a3, a4, a5],
        }
    }
}

impl TryFrom<&str> for MacAddress {
    type Error = ();
    fn try_from(string: &str) -> Result<MacAddress, Self::Error> {
        let mut address = [0u8; 6];
        for (i, part) in string.split(':').enumerate() {
            if i > 5 {
                break;
            }
            address[i] = u8::from_str_radix(part, 16).map_err(|_| ())?;
        }
        Ok(MacAddress { address })
    }
}

impl ::core::fmt::Display for MacAddress {
    fn fmt(&self, f: &mut ::core::fmt::Formatter) -> ::core::fmt::Result {
        write!(
            f,
            "{:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}",
            self.address[0],
            self.address[1],
            self.address[2],
            self.address[3],
            self.address[4],
            self.address[5],
        )
    }
}

#[derive(Copy, Clone, PartialOrd, PartialEq)]
pub enum OnWakeOnLan {
    InvokeInterrupt,
    Ignore,
}

#[derive(Copy, Clone, PartialOrd, PartialEq)]
pub enum OnPingRequest {
    Respond,
    Ignore,
}

/// Use [TransmissionMode::PPoE] when talking
/// to an ADSL modem. Otherwise use [TransmissionMode::Ethernet]
#[derive(Copy, Clone, PartialOrd, PartialEq)]
pub enum ConnectionType {
    PPoE,
    Ethernet,
}

#[derive(Copy, Clone, PartialOrd, PartialEq)]
pub enum ArpResponses {
    Cache,
    DropAfterUse,
}

pub struct UninitializedSocket(Socket);
pub struct TcpSocket(Socket);
pub struct UdpSocket(Socket);

pub struct W5500<'a, PinError: Send + Debug> {
    local_port: u16,
    chip_select: &'a mut (dyn OutputPin<Error = PinError> + Send),
    sockets: u8, // each bit represents whether the corresponding socket is available for take
}

impl<'b, 'a, PinError: Send + Debug> W5500<'a, PinError> {
    pub fn new(chip_select: &mut (dyn OutputPin<Error = PinError> + Send)) -> W5500<PinError> {
        W5500 {
            local_port: 40000,
            chip_select,
            sockets: 0xFF,
        }
    }

    pub async fn with_initialisation<'c, SPI: SpiBus>(
        chip_select: &'a mut (dyn OutputPin<Error = PinError> + Send),
        spi: &'c mut SPI,
        wol: OnWakeOnLan,
        ping: OnPingRequest,
        mode: ConnectionType,
        arp: ArpResponses,
    ) -> Result<W5500<'a, PinError>, SPI::Error> {
        let mut w5500 = Self::new(chip_select);
        {
            let mut w5500_active = w5500.activate(spi)?;
            unsafe {
                w5500_active.reset().await?;
            }
            w5500_active
                .update_operation_mode(wol, ping, mode, arp)
                .await?;
        }
        Ok(w5500)
    }

    pub fn take_socket(&mut self, socket: Socket) -> Option<UninitializedSocket> {
        let mask = 0x01 << socket.number();
        if self.sockets & mask == mask {
            self.sockets &= !mask;
            Some(UninitializedSocket(socket))
        } else {
            None
        }
    }

    pub fn activate<'c, SPI: SpiBus>(
        &'b mut self,
        spi: &'c mut SPI,
    ) -> Result<ActiveW5500<'b, 'a, 'c, SPI, PinError>, SPI::Error> {
        Ok(ActiveW5500(self, spi))
    }
}

pub struct ActiveW5500<'a, 'b: 'a, 'c, SPI: SpiBus, PinError: Send + Debug>(
    &'a mut W5500<'b, PinError>,
    &'c mut SPI,
);

impl<SPI: SpiBus, PinError: Send + Debug> ActiveW5500<'_, '_, '_, SPI, PinError> {
    pub fn take_socket(&mut self, socket: Socket) -> Option<UninitializedSocket> {
        self.0.take_socket(socket)
    }

    pub async fn update_operation_mode(
        &mut self,
        wol: OnWakeOnLan,
        ping: OnPingRequest,
        mode: ConnectionType,
        arp: ArpResponses,
    ) -> Result<(), SPI::Error> {
        let mut value = 0x00;

        if let OnWakeOnLan::InvokeInterrupt = wol {
            value |= 1 << 5;
        }

        if let OnPingRequest::Ignore = ping {
            value |= 1 << 4;
        }

        if let ConnectionType::PPoE = mode {
            value |= 1 << 3;
        }

        if let ArpResponses::DropAfterUse = arp {
            value |= 1 << 1;
        }

        self.write_u8(Register::CommonRegister(0x00_00_u16), value)
            .await
    }

    pub async fn set_gateway(&mut self, gateway: IpAddress) -> Result<(), SPI::Error> {
        self.write_to(Register::CommonRegister(0x00_01_u16), &gateway.address)
            .await
    }

    pub async fn set_subnet(&mut self, subnet: IpAddress) -> Result<(), SPI::Error> {
        self.write_to(Register::CommonRegister(0x00_05_u16), &subnet.address)
            .await
    }

    pub async fn set_mac(&mut self, mac: MacAddress) -> Result<(), SPI::Error> {
        self.write_to(Register::CommonRegister(0x00_09_u16), &mac.address)
            .await
    }

    pub async fn set_ip(&mut self, ip: IpAddress) -> Result<(), SPI::Error> {
        self.write_to(Register::CommonRegister(0x00_0F_u16), &ip.address)
            .await
    }

    pub async fn read_ip(&mut self, register: Register) -> Result<IpAddress, SPI::Error> {
        let mut ip = IpAddress::default();
        self.read_from(register, &mut ip.address).await?;
        Ok(ip)
    }

    /// # Safety
    ///
    /// This is unsafe because it cannot set taken sockets back to be uninitialized
    /// It assumes, none of the old sockets will used anymore. Otherwise that socket
    /// will have undefined behavior.
    pub async unsafe fn reset(&mut self) -> Result<(), SPI::Error> {
        self.write_u8(
            Register::CommonRegister(0x00_00_u16),
            0b1000_0000, // Mode Register (force reset)
        )
        .await?;
        self.0.sockets = 0xFF;
        Ok(())
    }

    pub async fn has_link(&mut self) -> Result<bool, SPI::Error> {
        let mut state = [0u8; 1];
        self.read_from(Register::CommonRegister(0x00_2E_u16), &mut state)
            .await?;
        Ok(state[0] & ((1 << 0) as u8) != 0)
    }

    async fn is_interrupt_set(
        &mut self,
        socket: Socket,
        interrupt: Interrupt,
    ) -> Result<bool, SPI::Error> {
        let mut state = [0u8; 1];
        self.read_from(socket.at(SocketRegister::Interrupt), &mut state)
            .await?;
        Ok(state[0] & interrupt as u8 != 0)
    }

    pub async fn reset_interrupt(
        &mut self,
        socket: Socket,
        interrupt: Interrupt,
    ) -> Result<(), SPI::Error> {
        self.write_u8(socket.at(SocketRegister::Interrupt), interrupt as u8)
            .await
    }

    pub async fn socket_status(&mut self, socket: &Socket) -> Result<SocketStatus, SPI::Error> {
        self.read_u8(socket.at(SocketRegister::Status))
            .await
            .map(SocketStatus::from_u8)
    }

    async fn read_u8(&mut self, register: Register) -> Result<u8, SPI::Error> {
        let mut buffer = [0u8; 1];
        self.read_from(register, &mut buffer).await?;
        Ok(buffer[0])
    }

    async fn read_u16(&mut self, register: Register) -> Result<u16, SPI::Error> {
        let mut buffer = [0u8; 2];
        self.read_from(register, &mut buffer).await?;
        Ok(BigEndian::read_u16(&buffer))
    }

    async fn read_from(&mut self, register: Register, target: &mut [u8]) -> Result<(), SPI::Error> {
        self.chip_select();
        let mut request = [
            0_u8,
            0_u8,
            register.control_byte() | COMMAND_READ | VARIABLE_DATA_LENGTH,
        ];
        BigEndian::write_u16(&mut request[..2], register.address());

        let result = match self.write_header(&request).await {
            Ok(_) => self.read_bytes(target).await,
            Err(e) => Err(e),
        };
        self.chip_deselect();
        result
    }

    async fn read_bytes(&mut self, bytes: &mut [u8]) -> Result<(), SPI::Error> {
        self.1.read(bytes).await?;
        Ok(())
    }

    async fn write_u8(&mut self, register: Register, value: u8) -> Result<(), SPI::Error> {
        self.write_to(register, &[value]).await
    }

    async fn write_u16(&mut self, register: Register, value: u16) -> Result<(), SPI::Error> {
        let mut data = [0u8; 2];
        BigEndian::write_u16(&mut data, value);
        self.write_to(register, &data).await
    }

    async fn write_to(&mut self, register: Register, data: &[u8]) -> Result<(), SPI::Error> {
        self.chip_select();
        let mut request = [
            0_u8,
            0_u8,
            register.control_byte() | COMMAND_WRITE | VARIABLE_DATA_LENGTH,
        ];
        BigEndian::write_u16(&mut request[..2], register.address());
        let result = match self.write_header(&request).await {
            Ok(_) => self.write_bytes(data).await,
            Err(e) => Err(e),
        };

        self.chip_deselect();
        result
    }

    async fn write_bytes(&mut self, bytes: &[u8]) -> Result<(), SPI::Error> {
        let mut padding = [0; 512]; // TODO: REMOVE
        self.1.transfer(&mut padding[..bytes.len()], bytes).await // TODO: embassy spi.write_and_discard_rx()???
                                                                  // https://github.com/embassy-rs/embassy/issues/637
    }

    async fn write_header(&mut self, bytes: &[u8; 3]) -> Result<(), SPI::Error> {
        let mut padding = [0; 3];
        self.1.transfer(&mut padding, bytes).await
    }

    fn chip_select(&mut self) {
        let _ = self.0.chip_select.set_low();
    }

    fn chip_deselect(&mut self) {
        let _ = self.0.chip_select.set_high();
    }
}

pub trait IntoTcpSocket {
    async fn try_into_tcp_server_socket(self, port: u16) -> Result<TcpSocket, UninitializedSocket>;
    async fn try_into_tcp_client_socket(self, ip: IpAddress, port: u16) -> Result<TcpSocket, UninitializedSocket>;
}

impl<'a, 'w5500: 'a, 'pin: 'w5500, 'spi: 'a, SPI, PinError: Send + Debug> IntoTcpSocket
    for (
        &'a mut ActiveW5500<'w5500, 'pin, 'spi, SPI, PinError>,
        UninitializedSocket,
    )
where
    PinError: 'pin,
    SPI: 'spi + SpiBus + Send,
    SPI::Error: Send,
{

    async fn try_into_tcp_server_socket(self, port: u16) -> Result<TcpSocket, UninitializedSocket> {
        let (w5500, UninitializedSocket(socket)) = self;

        async move {
            w5500.reset_interrupt(socket, Interrupt::SendOk).await?;

            // set the port to use
            w5500
                .write_u16(socket.at(SocketRegister::LocalPort), port)
                .await?;

            // open the TCP socket
            // the Command register directly follows the Mode register so we can write to both at once
            w5500
                .write_to(
                    socket.at(SocketRegister::Mode),
                    &[Protocol::TCP as u8, SocketCommand::Open as u8],
                )
                .await?;

            // wait for the socket to be ready
            while w5500.socket_status(&socket).await? != SocketStatus::Init {
                yield_now().await;
            }

            // start listening
            w5500
                .write_u8(
                    socket.at(SocketRegister::Command),
                    SocketCommand::Listen as u8,
                )
                .await?;

            // wait for the socket to start listening
            while w5500.socket_status(&socket).await? != SocketStatus::Listen {
                yield_now().await;
            }

            Ok(TcpSocket(socket)) as Result<_, SPI::Error>
        }
        .map_err(move |_: SPI::Error| UninitializedSocket(socket)).await
    }

    async fn try_into_tcp_client_socket(self, ip: IpAddress, port: u16) -> Result<TcpSocket, UninitializedSocket> {
        let (w5500, UninitializedSocket(socket)) = self;

        async move {
            w5500.reset_interrupt(socket, Interrupt::SendOk).await?;

            let rtr_read = w5500
                .read_u16(socket.at(SocketRegister::RetryTimeRegister))
                .await?;
            let rcr_read = w5500
                .read_u8(socket.at(SocketRegister::RetryCountRegister))
                .await?;

            w5500
                .write_u16(socket.at(SocketRegister::RetryTimeRegister), 200)
                .await?;
            w5500
                .write_u8(socket.at(SocketRegister::RetryCountRegister), 3)
                .await?;

            // set local port automatically, and increment it for next use
            w5500
                .write_u16(socket.at(SocketRegister::LocalPort), w5500.0.local_port)
                .await?;
            w5500.0.local_port += 1;

            // set remote addr to connect to
            w5500
                .write_u16(socket.at(SocketRegister::DestinationPort), port)
                .await?;
            w5500
                .write_to(socket.at(SocketRegister::DestinationIp), &ip.address)
                .await?;

            // open the TCP socket
            // the Command register directly follows the Mode register so we can write to both at once
            w5500
                .write_to(
                    socket.at(SocketRegister::Mode),
                    &[
                        Protocol::TCP as u8,       // Socket Mode Register
                        SocketCommand::Open as u8, // Socket Command Register
                    ],
                )
                .await?;

            // wait for the socket to be ready
            while w5500.socket_status(&socket).await? != SocketStatus::Init {
                yield_now().await;
            }

            // issue the connect command
            w5500
                .write_u8(
                    socket.at(SocketRegister::Command),
                    SocketCommand::Connect as u8,
                )
                .await?;

            // wait for the connection to be established
            while w5500.socket_status(&socket).await? != SocketStatus::Established {
                yield_now().await;
            }

            w5500
                .write_u16(socket.at(SocketRegister::RetryTimeRegister), rtr_read)
                .await?;
            w5500
                .write_u8(socket.at(SocketRegister::RetryCountRegister), rcr_read)
                .await?;

            Ok(TcpSocket(socket)) as Result<_, SPI::Error>
        }
        .map_err(move |_: SPI::Error| UninitializedSocket(socket)).await
    }
}

pub trait IntoUdpSocket {
    async fn try_into_udp_server_socket(self, port: u16) -> Result<UdpSocket, UninitializedSocket>;
    async fn try_into_udp_client_socket(self, port: u16) -> Result<UdpSocket, UninitializedSocket>;
}

impl<'a, 'w5500: 'a, 'pin: 'w5500, 'spi: 'a, SPI: SpiBus + Send, PinError: Send + Debug + 'static>
    IntoUdpSocket
    for (
        &'a mut ActiveW5500<'w5500, 'pin, 'spi, SPI, PinError>,
        UninitializedSocket,
    )
where
    PinError: 'pin,
    SPI: 'spi + SpiBus + Send,
    SPI::Error: Send,
{

    async fn try_into_udp_server_socket(self, port: u16) -> Result<UdpSocket, UninitializedSocket> {
        let (w5500, UninitializedSocket(socket)) = self;

        async move {
            w5500.reset_interrupt(socket, Interrupt::SendOk).await?;

            w5500
                .write_u16(socket.at(SocketRegister::LocalPort), port)
                .await?;

            w5500
                .write_to(
                    socket.at(SocketRegister::Mode),
                    &[
                        Protocol::UDP as u8,       // Socket Mode Register
                        SocketCommand::Open as u8, // Socket Command Register
                    ],
                )
                .await?;

            Ok(UdpSocket(socket))
        }
        .map_err(move |_: SPI::Error| UninitializedSocket(socket)).await
    }

    async fn try_into_udp_client_socket(self, port: u16) -> Result<UdpSocket, UninitializedSocket> {
        let (w5500, UninitializedSocket(socket)) = self;

        async move {
            w5500.reset_interrupt(socket, Interrupt::SendOk).await?;

            w5500
                .write_u16(socket.at(SocketRegister::DestinationPort), port)
                .await?;

            w5500
                .write_to(
                    socket.at(SocketRegister::Mode),
                    &[Protocol::UDP as u8, SocketCommand::Open as u8],
                )
                .await?;

            Ok(UdpSocket(socket))
        }
        .map_err(move |_: SPI::Error| UninitializedSocket(socket)).await
    }
}

pub trait Tcp<E> {
    type Error;

    async fn receive<'a>(&'a mut self, target_buffer: &'a mut [u8]) -> Result<Option<(IpAddress, u16, usize)>, Self::Error>;
    async fn blocking_send<'a>(&'a mut self, data: &'a [u8]) -> Result<(), Self::Error>;
    async fn disconnect(&mut self) -> Result<(), Self::Error>;
    async fn reconnect(&mut self) -> Result<(), Self::Error>;
    async fn is_connected(&mut self) -> Result<bool, Self::Error>;
}

impl<'b, 'w5500: 'b, 'pin: 'w5500, 'spi: 'b, SPI: SpiBus + Send + 'static, PinError: Send + Debug + 'static>
    Tcp<SPI::Error>
    for (
        &'b mut ActiveW5500<'w5500, 'pin, 'spi, SPI, PinError>,
        &TcpSocket,
    )
where
    PinError: 'pin,
    SPI: 'spi + SpiBus + Send,
{
    type Error = SPI::Error;

    async fn receive<'a>(&'a mut self, destination: &'a mut [u8]) -> Result<Option<(IpAddress, u16, usize)>, SPI::Error> {
        let (w5500, TcpSocket(socket)) = self;

        if w5500.socket_status(socket).await? != SocketStatus::Established {
            return Ok(None);
        }

        if w5500
            .read_u8(socket.at(SocketRegister::InterruptMask))
            .await?
            & 1 << 2
            == 0
        // if !RECV
        {
            return Ok(None);
        }

        // wait until the amount of bytes we received stops growing (we're done receiving)
        let receive_size = loop {
            let s0 = w5500
                .read_u16(socket.at(SocketRegister::RxReceivedSize))
                .await?;
            let s1 = w5500
                .read_u16(socket.at(SocketRegister::RxReceivedSize))
                .await?;
            if s0 == s1 {
                break s0 as u16;
            }
        };

        if receive_size > 0 {
            let read_pointer = w5500
                .read_u16(socket.at(SocketRegister::RxReadPointer))
                .await?;

            let ip = w5500
                .read_ip(socket.at(SocketRegister::DestinationIp))
                .await?;
            let port = w5500
                .read_u16(socket.at(SocketRegister::DestinationPort))
                .await?;

            let data_length = destination.len().min(receive_size as usize);

            w5500
                .read_from(
                    socket.rx_register_at(read_pointer),
                    &mut destination[..data_length],
                )
                .await?;

            // reset
            w5500
                .write_u16(
                    socket.at(SocketRegister::RxReadPointer),
                    read_pointer.overflowing_add(receive_size).0 as u16,
                )
                .await?;

            w5500
                .write_u8(
                    socket.at(SocketRegister::Command),
                    SocketCommand::Recv as u8,
                )
                .await?;

            Ok(Some((ip, port, receive_size as usize)))
        } else {
            Ok(None)
        }
    }

    async fn blocking_send<'a>(&'a mut self, data: &'a [u8]) -> Result<(), SPI::Error> {
        let (w5500, TcpSocket(socket)) = self;

        let data_length = data.len() as u16;

        // TODO: check if there's enough space in transmit buffer
        // let tx_buffer_size = w5500
        //     .read_u16(socket.at(SocketRegister::TransmitBuffer))
        //     .await?;

        let rx_read_pointer = w5500
            .read_u16(socket.at(SocketRegister::TxReadPointer))
            .await?;

        w5500
            .write_to(
                socket.tx_register_at(rx_read_pointer),
                &data[..data_length as usize],
            )
            .await?;

        // reset
        let tx_write_pointer = w5500
            .read_u16(socket.at(SocketRegister::TxWritePointer))
            .await?;

        w5500
            .write_u16(
                socket.at(SocketRegister::TxWritePointer),
                tx_write_pointer.overflowing_add(data_length as u16).0,
            )
            .await?;

        w5500
            .write_u8(
                socket.at(SocketRegister::Command),
                SocketCommand::Send as u8,
            )
            .await?;

        for _ in 0..0xFFFF {
            // wait until sent
            if w5500.is_interrupt_set(*socket, Interrupt::SendOk).await? {
                w5500.reset_interrupt(*socket, Interrupt::SendOk).await?;
                break;
            }

            yield_now().await;
        }

        Ok(())
    }

    async fn disconnect(&mut self) -> Result<(), SPI::Error> {
        let (w5500, TcpSocket(socket)) = self;

        w5500
            .write_u8(
                socket.at(SocketRegister::Command),
                SocketCommand::Disconnect as u8,
            )
            .await?;

        while w5500.socket_status(socket).await? == SocketStatus::FinWait {
            yield_now().await;
        }

        while w5500.socket_status(socket).await? == SocketStatus::Closed {
            yield_now().await;
        }

        Ok(())

    }

    async fn reconnect(&mut self) -> Result<(), SPI::Error> {
        // TODO: is this really what reconnect() should be doing?
        if self.is_connected().await? {
            return Ok(());
        }

        let (w5500, TcpSocket(socket)) = self;

        let rtr_read = w5500
            .read_u16(socket.at(SocketRegister::RetryTimeRegister))
            .await?;
        let rcr_read = w5500
            .read_u8(socket.at(SocketRegister::RetryCountRegister))
            .await?;

        w5500
            .write_u16(socket.at(SocketRegister::RetryTimeRegister), 200)
            .await?;
        w5500
            .write_u8(socket.at(SocketRegister::RetryCountRegister), 2)
            .await?;

        let mut local_port: u16 = w5500.read_u16(socket.at(SocketRegister::LocalPort)).await?;
        if local_port.overflowing_add(1).1 {
            local_port = 40000;
        } else {
            local_port += 1;
        }

        w5500
            .write_u8(
                socket.at(SocketRegister::Interrupt),
                Interrupt::SendOk as u8,
            )
            .await?;
        w5500
            .write_u16(socket.at(SocketRegister::LocalPort), local_port)
            .await?;

        w5500
            .write_to(
                socket.at(SocketRegister::Mode),
                &[
                    Protocol::TCP as u8,       // Socket Mode Register
                    SocketCommand::Open as u8, // Socket Command Register
                ],
            )
            .await?;

        while w5500.socket_status(socket).await? != SocketStatus::Init {
            yield_now().await;
        }

        w5500
            .write_to(
                socket.at(SocketRegister::Mode),
                &[Protocol::TCP as u8, SocketCommand::Connect as u8],
            )
            .await?;

        while w5500.socket_status(socket).await? != SocketStatus::Established {
            yield_now().await;
        }

        w5500
            .write_u16(socket.at(SocketRegister::RetryTimeRegister), rtr_read)
            .await?;
        w5500
            .write_u8(socket.at(SocketRegister::RetryCountRegister), rcr_read)
            .await?;

        Ok(())
    }

    async fn is_connected(&mut self) -> Result<bool, SPI::Error> {
        let (w5500, TcpSocket(socket)) = self;
        Ok(w5500.socket_status(socket).await? == SocketStatus::Established)
    }
}

pub trait Udp<E> {
    type Error<'a>;
    type ReceiveFut<'a>
    where
        Self: 'a;
    type BlockingSendFut<'a>
    where
        Self: 'a;

    fn receive<'a>(&'a mut self, target_buffer: &'a mut [u8]) -> Self::ReceiveFut<'a>;
    fn blocking_send<'a>(
        &'a mut self,
        host: &'a IpAddress,
        host_port: u16,
        data: &'a [u8],
    ) -> Self::BlockingSendFut<'a>;
}

impl<'b, 'w5500: 'b, 'pin: 'w5500, 'spi: 'b, SPI: SpiBus + Send, PinError: Send + Debug>
    Udp<SPI::Error>
    for (
        &'b mut ActiveW5500<'w5500, 'pin, 'spi, SPI, PinError>,
        &UdpSocket,
    )
where
    PinError: 'pin,
    SPI: 'spi + SpiBus + Send,
{
    type Error<'a> = SPI::Error;
    type ReceiveFut<'a> =
        impl Future<Output = Result<Option<(IpAddress, u16, usize)>, Self::Error<'a>>> where Self: 'a;
    type BlockingSendFut<'a> = impl Future<Output = Result<(), Self::Error<'a>>> where Self: 'a;

    fn receive<'a>(&'a mut self, destination: &'a mut [u8]) -> Self::ReceiveFut<'a> {
        let (w5500, UdpSocket(socket)) = self;

        async move {
            if w5500
                .read_u8(socket.at(SocketRegister::InterruptMask))
                .await?
                & 1 << 2
                == 0
            // if !RECV
            {
                return Ok(None);
            }

            // wait until the amount of bytes we received stops growing (we're done receiving)
            let receive_size = loop {
                let s0 = w5500
                    .read_u16(socket.at(SocketRegister::RxReceivedSize))
                    .await?;
                let s1 = w5500
                    .read_u16(socket.at(SocketRegister::RxReceivedSize))
                    .await?;
                if s0 == s1 {
                    break s0 as usize;
                }
            };

            if receive_size >= 8 {
                let read_pointer = w5500
                    .read_u16(socket.at(SocketRegister::RxReadPointer))
                    .await?;

                // |<-- read_pointer                                read_pointer + received_size -->|
                // |Destination IP Address | Destination Port | Byte Size of DATA | Actual DATA ... |
                // |   --- 4 Bytes ---     |  --- 2 Bytes --- |  --- 2 Bytes ---  |      ....       |

                let ip = w5500.read_ip(socket.rx_register_at(read_pointer)).await?;
                let port = w5500
                    .read_u16(socket.rx_register_at(read_pointer + 4))
                    .await?;

                let data_length = destination.len().min(
                    w5500
                        .read_u16(socket.rx_register_at(read_pointer + 6))
                        .await? as usize,
                );

                w5500
                    .read_from(
                        socket.rx_register_at(read_pointer + 8),
                        &mut destination[..data_length],
                    )
                    .await?;

                // reset
                w5500
                    .write_u16(
                        socket.at(SocketRegister::RxReadPointer),
                        read_pointer + receive_size as u16,
                    )
                    .await?;

                w5500
                    .write_u8(
                        socket.at(SocketRegister::Command),
                        SocketCommand::Recv as u8,
                    )
                    .await?;

                Ok(Some((ip, port, data_length)))
            } else {
                Ok(None)
            }
        }
    }

    fn blocking_send<'a>(
        &'a mut self,
        host: &'a IpAddress,
        host_port: u16,
        data: &'a [u8],
    ) -> Self::BlockingSendFut<'a> {
        let (w5500, UdpSocket(socket)) = self;
        async move {
            {
                let local_port = w5500.read_u16(socket.at(SocketRegister::LocalPort)).await?;
                let local_port = local_port.to_be_bytes();
                let host_port = host_port.to_be_bytes();

                w5500
                    .write_to(
                        socket.at(SocketRegister::LocalPort),
                        &[
                            local_port[0],
                            local_port[1], // local port u16
                            0x00,
                            0x00,
                            0x00,
                            0x00,
                            0x00,
                            0x00, // destination mac
                            host.address[0],
                            host.address[1],
                            host.address[2],
                            host.address[3], // target IP
                            host_port[0],
                            host_port[1], // destination port (5354)
                        ],
                    )
                    .await?;
            }

            let data_length = data.len() as u16;
            {
                let data_length = data_length.to_be_bytes();

                // TODO why write [0x00, 0x00] at TxReadPointer at all?
                // TODO Is TxWritePointer not sufficient enough?
                w5500
                    .write_to(
                        socket.at(SocketRegister::TxReadPointer),
                        &[0x00, 0x00, data_length[0], data_length[1]],
                    )
                    .await?;
            }

            w5500
                .write_to(
                    socket.tx_register_at(0x00_00),
                    &data[..data_length as usize],
                )
                .await?;

            w5500
                .write_to(
                    socket.at(SocketRegister::Command),
                    &[SocketCommand::Send as u8],
                )
                .await?;

            for _ in 0..0xFFFF {
                // wait until sent
                if w5500.is_interrupt_set(*socket, Interrupt::SendOk).await? {
                    w5500.reset_interrupt(*socket, Interrupt::SendOk).await?;
                    break;
                }

                yield_now().await;
            }
            // restore listen state
            /*
            self.network
                .listen_udp(self.spi, SOCKET_UDP, SOCKET_UDP_PORT)
            */
            w5500
                .write_to(
                    socket.at(SocketRegister::Mode),
                    &[
                        Protocol::UDP as u8,       // Socket Mode Register
                        SocketCommand::Open as u8, // Socket Command Register
                    ],
                )
                .await?;
            Ok(())
        }
    }
}

#[repr(u8)]
#[derive(Copy, Clone, PartialEq, Debug)]
pub enum SocketRegister {
    Mode = 0x0000,
    Command = 0x0001,
    Interrupt = 0x0002,
    Status = 0x0003,
    LocalPort = 0x0004,
    DestinationMac = 0x0006,
    DestinationIp = 0x000C,
    DestinationPort = 0x0010,
    MaxSegmentSize = 0x0012,
    // Reserved 0x0014
    TypeOfService = 0x0015,
    TimeToLive = 0x0016,
    // Reserved 0x0017 - 0x001D
    RetryTimeRegister = 0x0019,
    RetryCountRegister = 0x001B,
    ReceiveBuffer = 0x001E,
    TransmitBuffer = 0x001F,
    TxFreeSize = 0x0020,
    TxReadPointer = 0x0022,
    TxWritePointer = 0x0024,
    RxReceivedSize = 0x0026,
    RxReadPointer = 0x0028,
    RxWritePointer = 0x002A,
    InterruptMask = 0x002C,
    FragmentOffset = 0x002D,
    KeepAliveTimer = 0x002F,
    // Reserved 0x0030 - 0xFFFF
}

#[repr(u8)]
#[derive(Copy, Clone, PartialEq, Debug)]
pub enum Interrupt {
    SendOk = 1 << 4,
    Timeout = 1 << 3,
    Received = 1 << 2,
    Disconnected = 1 << 1,
    Connected = 1, // 1 << 0
}

#[repr(u8)]
#[derive(Copy, Clone, PartialEq, Debug)]
pub enum Protocol {
    TCP = 0b0001,
    UDP = 0b0010,
    MACRAW = 0b0100,
}

#[repr(u8)]
#[derive(Copy, Clone, PartialEq, Debug)]
pub enum SocketCommand {
    Open = 0x01,
    Listen = 0x02,
    Connect = 0x04,
    Disconnect = 0x08,
    Close = 0x10,
    Send = 0x20,
    SendMac = 0x21,
    SendKeep = 0x22,
    Recv = 0x40,
}

#[derive(Copy, Clone, PartialEq, Debug)]
pub enum SocketStatus {
    Closed,
    Init,
    Listen,
    SynSent,
    SynRecv,
    Established,
    FinWait,
    Closing,
    TimeWait,
    CloseWait,
    LastAck,
    SockUdp,
    MacRaw,

    Unknown(u8),
}

impl SocketStatus {
    fn from_u8(n: u8) -> Self {
        match n {
            0x00 => Self::Closed,
            0x13 => Self::Init,
            0x14 => Self::Listen,
            0x15 => Self::SynSent,
            0x16 => Self::SynRecv,
            0x17 => Self::Established,
            0x18 => Self::FinWait,
            0x1a => Self::Closing,
            0x1b => Self::TimeWait,
            0x1c => Self::CloseWait,
            0x1d => Self::LastAck,
            0x22 => Self::SockUdp,
            0x42 => Self::MacRaw,

            n => Self::Unknown(n),
        }
    }
}

#[derive(Copy, Clone, PartialEq, PartialOrd, Debug)]
pub enum Socket {
    Socket0,
    Socket1,
    Socket2,
    Socket3,
    Socket4,
    Socket5,
    Socket6,
    Socket7,
}

impl Socket {
    pub fn number(self) -> usize {
        match self {
            Socket::Socket0 => 0,
            Socket::Socket1 => 1,
            Socket::Socket2 => 2,
            Socket::Socket3 => 3,
            Socket::Socket4 => 4,
            Socket::Socket5 => 5,
            Socket::Socket6 => 6,
            Socket::Socket7 => 7,
        }
    }

    fn tx_register_at(self, address: u16) -> Register {
        match self {
            Socket::Socket0 => Register::Socket0TxBuffer(address),
            Socket::Socket1 => Register::Socket1TxBuffer(address),
            Socket::Socket2 => Register::Socket2TxBuffer(address),
            Socket::Socket3 => Register::Socket3TxBuffer(address),
            Socket::Socket4 => Register::Socket4TxBuffer(address),
            Socket::Socket5 => Register::Socket5TxBuffer(address),
            Socket::Socket6 => Register::Socket6TxBuffer(address),
            Socket::Socket7 => Register::Socket7TxBuffer(address),
        }
    }

    fn rx_register_at(self, address: u16) -> Register {
        match self {
            Socket::Socket0 => Register::Socket0RxBuffer(address),
            Socket::Socket1 => Register::Socket1RxBuffer(address),
            Socket::Socket2 => Register::Socket2RxBuffer(address),
            Socket::Socket3 => Register::Socket3RxBuffer(address),
            Socket::Socket4 => Register::Socket4RxBuffer(address),
            Socket::Socket5 => Register::Socket5RxBuffer(address),
            Socket::Socket6 => Register::Socket6RxBuffer(address),
            Socket::Socket7 => Register::Socket7RxBuffer(address),
        }
    }

    fn register_at(self, address: u16) -> Register {
        match self {
            Socket::Socket0 => Register::Socket0Register(address),
            Socket::Socket1 => Register::Socket1Register(address),
            Socket::Socket2 => Register::Socket2Register(address),
            Socket::Socket3 => Register::Socket3Register(address),
            Socket::Socket4 => Register::Socket4Register(address),
            Socket::Socket5 => Register::Socket5Register(address),
            Socket::Socket6 => Register::Socket6Register(address),
            Socket::Socket7 => Register::Socket7Register(address),
        }
    }

    fn at(self, register: SocketRegister) -> Register {
        self.register_at(register as u16)
    }
}

#[derive(Copy, Clone, PartialEq, Debug)]
pub enum Register {
    CommonRegister(u16),

    Socket0Register(u16),
    Socket0TxBuffer(u16),
    Socket0RxBuffer(u16),

    Socket1Register(u16),
    Socket1TxBuffer(u16),
    Socket1RxBuffer(u16),

    Socket2Register(u16),
    Socket2TxBuffer(u16),
    Socket2RxBuffer(u16),

    Socket3Register(u16),
    Socket3TxBuffer(u16),
    Socket3RxBuffer(u16),

    Socket4Register(u16),
    Socket4TxBuffer(u16),
    Socket4RxBuffer(u16),

    Socket5Register(u16),
    Socket5TxBuffer(u16),
    Socket5RxBuffer(u16),

    Socket6Register(u16),
    Socket6TxBuffer(u16),
    Socket6RxBuffer(u16),

    Socket7Register(u16),
    Socket7TxBuffer(u16),
    Socket7RxBuffer(u16),
}

impl Register {
    fn control_byte(self) -> u8 {
        #[allow(clippy::inconsistent_digit_grouping)]
        match self {
            Register::CommonRegister(_) => 0b00000_000,

            Register::Socket0Register(_) => 0b00001_000,
            Register::Socket0TxBuffer(_) => 0b00010_000,
            Register::Socket0RxBuffer(_) => 0b00011_000,

            Register::Socket1Register(_) => 0b00101_000,
            Register::Socket1TxBuffer(_) => 0b00110_000,
            Register::Socket1RxBuffer(_) => 0b00111_000,

            Register::Socket2Register(_) => 0b01001_000,
            Register::Socket2TxBuffer(_) => 0b01010_000,
            Register::Socket2RxBuffer(_) => 0b01011_000,

            Register::Socket3Register(_) => 0b01101_000,
            Register::Socket3TxBuffer(_) => 0b01110_000,
            Register::Socket3RxBuffer(_) => 0b01111_000,

            Register::Socket4Register(_) => 0b10001_000,
            Register::Socket4TxBuffer(_) => 0b10010_000,
            Register::Socket4RxBuffer(_) => 0b10011_000,

            Register::Socket5Register(_) => 0b10101_000,
            Register::Socket5TxBuffer(_) => 0b10110_000,
            Register::Socket5RxBuffer(_) => 0b10111_000,

            Register::Socket6Register(_) => 0b11001_000,
            Register::Socket6TxBuffer(_) => 0b11010_000,
            Register::Socket6RxBuffer(_) => 0b11011_000,

            Register::Socket7Register(_) => 0b11101_000,
            Register::Socket7TxBuffer(_) => 0b11110_000,
            Register::Socket7RxBuffer(_) => 0b11111_000,
        }
    }

    fn address(self) -> u16 {
        match self {
            Register::CommonRegister(address) => address,

            Register::Socket0Register(address) => address,
            Register::Socket0TxBuffer(address) => address,
            Register::Socket0RxBuffer(address) => address,

            Register::Socket1Register(address) => address,
            Register::Socket1TxBuffer(address) => address,
            Register::Socket1RxBuffer(address) => address,

            Register::Socket2Register(address) => address,
            Register::Socket2TxBuffer(address) => address,
            Register::Socket2RxBuffer(address) => address,

            Register::Socket3Register(address) => address,
            Register::Socket3TxBuffer(address) => address,
            Register::Socket3RxBuffer(address) => address,

            Register::Socket4Register(address) => address,
            Register::Socket4TxBuffer(address) => address,
            Register::Socket4RxBuffer(address) => address,

            Register::Socket5Register(address) => address,
            Register::Socket5TxBuffer(address) => address,
            Register::Socket5RxBuffer(address) => address,

            Register::Socket6Register(address) => address,
            Register::Socket6TxBuffer(address) => address,
            Register::Socket6RxBuffer(address) => address,

            Register::Socket7Register(address) => address,
            Register::Socket7TxBuffer(address) => address,
            Register::Socket7RxBuffer(address) => address,
        }
    }
}
