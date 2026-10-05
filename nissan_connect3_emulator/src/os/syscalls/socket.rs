use crate::emulator::context::Context;
use crate::emulator::thread::{BlockReason, ThreadAction};
use unicorn_engine::{RegisterARM, Unicorn};

const MSG_DONTWAIT: u32 = 0x40;

fn is_emulated_socket(unicorn: &Unicorn<'_, Context>, socket_fd: u32) -> bool {
    unicorn
        .get_data()
        .inner
        .sys_calls_state
        .lock()
        .unwrap()
        .socket_fds
        .contains(&socket_fd)
}

fn block_for_receive(unicorn: &mut Unicorn<'_, Context>, socket_fd: u32, flags: u32) -> u32 {
    if !is_emulated_socket(unicorn, socket_fd) {
        return -9i32 as u32; // -EBADF
    }
    if flags & MSG_DONTWAIT != 0 {
        return -11i32 as u32; // -EAGAIN
    }
    unicorn
        .get_data()
        .set_action(ThreadAction::Block(BlockReason::SocketRead {
            fd: socket_fd,
            deadline: None,
        }));
    0u32
}

pub fn socket(
    unicorn: &mut Unicorn<'_, Context>,
    domain: u32,
    socket_type: u32,
    protocol: u32,
) -> u32 {
    log::trace!(
        "{:#x}: [{}] [SYSCALL] socket(domain = {:#x}, socket_type: {:#x}, protocol: {:#x}) [IN]",
        unicorn.reg_read(RegisterARM::PC).unwrap(),
        unicorn.get_data().inner.thread_id(),
        domain,
        socket_type,
        protocol,
    );

    // TODO: back these with real host sockets (or emulated netlink) once we
    // know which protocols the guests actually depend on. For now we allocate
    // fake fds so a socket fd never aliases a real file descriptor.
    let res = {
        let state = &mut unicorn.get_data().inner.sys_calls_state.lock().unwrap();
        let fd = state.next_socket_fd;
        state.next_socket_fd += 1;
        state.socket_fds.insert(fd);
        fd
    };

    log::trace!(
        "{:#x}: [{}] [SYSCALL] socket => {:#x}",
        unicorn.reg_read(RegisterARM::PC).unwrap(),
        unicorn.get_data().inner.thread_id(),
        res
    );

    res
}

pub fn connect(
    unicorn: &mut Unicorn<'_, Context>,
    socket_fd: u32,
    addr: u32,
    addr_len: u32,
) -> u32 {
    log::trace!(
        "{:#x}: [{}] [SYSCALL] connect(socket_fd = {:#x}, addr: {:#x}, addr_len: {:#x}) [IN]",
        unicorn.reg_read(RegisterARM::PC).unwrap(),
        unicorn.get_data().inner.thread_id(),
        socket_fd,
        addr,
        addr_len,
    );

    // TODO: implement
    let res = 0;

    log::trace!(
        "{:#x}: [{}] [SYSCALL] connect => {:#x}",
        unicorn.reg_read(RegisterARM::PC).unwrap(),
        unicorn.get_data().inner.thread_id(),
        res
    );

    res
}

pub fn bind(unicorn: &mut Unicorn<'_, Context>, socket_fd: u32, addr: u32, addr_len: u32) -> u32 {
    log::trace!(
        "{:#x}: [{}] [SYSCALL] bind(socket_fd = {:#x}, addr: {:#x}, addr_len: {:#x}) [IN]",
        unicorn.reg_read(RegisterARM::PC).unwrap(),
        unicorn.get_data().inner.thread_id(),
        socket_fd,
        addr,
        addr_len,
    );

    // TODO: implement
    let res = 0;

    log::trace!(
        "{:#x}: [{}] [SYSCALL] bind => {:#x}",
        unicorn.reg_read(RegisterARM::PC).unwrap(),
        unicorn.get_data().inner.thread_id(),
        res
    );

    res
}

pub fn setsockopt(
    unicorn: &mut Unicorn<'_, Context>,
    socket_fd: u32,
    level: u32,
    option_name: u32,
    option_value: u32,
    option_len: u32,
) -> u32 {
    log::trace!(
        "{:#x}: [{}] [SYSCALL] setsockopt(socket_fd = {:#x}, level: {:#x}, optname: {:#x}, optval: {:#x}, optlen: {:#x}) [IN]",
        unicorn.reg_read(RegisterARM::PC).unwrap(),
        unicorn.get_data().inner.thread_id(),
        socket_fd,
        level,
        option_name,
        option_value,
        option_len,
    );

    // TODO: implement
    let res = 0;

    log::trace!(
        "{:#x}: [{}] [SYSCALL] setsockopt => {:#x}",
        unicorn.reg_read(RegisterARM::PC).unwrap(),
        unicorn.get_data().inner.thread_id(),
        res
    );

    res
}

pub fn getsockname(
    unicorn: &mut Unicorn<'_, Context>,
    socket_fd: u32,
    addr: u32,
    addr_len: u32,
) -> u32 {
    log::trace!(
        "{:#x}: [{}] [SYSCALL] getsockname(socket_fd = {:#x}, addr: {:#x}, addr_len: {:#x}) [IN]",
        unicorn.reg_read(RegisterARM::PC).unwrap(),
        unicorn.get_data().inner.thread_id(),
        socket_fd,
        addr,
        addr_len,
    );

    // TODO: return a valid emulated socket address
    let res = 0;

    log::trace!(
        "{:#x}: [{}] [SYSCALL] getsockname => {:#x}",
        unicorn.reg_read(RegisterARM::PC).unwrap(),
        unicorn.get_data().inner.thread_id(),
        res
    );

    res
}

pub fn getsockopt(
    unicorn: &mut Unicorn<'_, Context>,
    socket_fd: u32,
    level: u32,
    option_name: u32,
    option_value: u32,
    option_len_addr: u32,
) -> u32 {
    log::trace!(
        "{:#x}: [{}] [SYSCALL] getsockopt(socket_fd = {:#x}, level: {:#x}, optname: {:#x}, optval: {:#x}, optlen: {:#x}) [IN]",
        unicorn.reg_read(RegisterARM::PC).unwrap(),
        unicorn.get_data().inner.thread_id(),
        socket_fd,
        level,
        option_name,
        option_value,
        option_len_addr,
    );

    // TODO: implement
    let res = 0;

    log::trace!(
        "{:#x}: [{}] [SYSCALL] getsockopt => {:#x}",
        unicorn.reg_read(RegisterARM::PC).unwrap(),
        unicorn.get_data().inner.thread_id(),
        res
    );

    res
}

pub fn send(
    unicorn: &mut Unicorn<'_, Context>,
    socket_fd: u32,
    buf: u32,
    len: u32,
    flags: u32,
) -> u32 {
    log::trace!(
        "{:#x}: [{}] [SYSCALL] send(socket_fd = {:#x}, buf: {:#x}, len: {:#x}, flags: {:#x}) [IN]",
        unicorn.reg_read(RegisterARM::PC).unwrap(),
        unicorn.get_data().inner.thread_id(),
        socket_fd,
        buf,
        len,
        flags,
    );

    // TODO: implement
    let res = 0;

    let mut buf2 = vec![0u8; len as usize];
    unicorn.mem_read(buf as u64, &mut buf2).unwrap();
    let str = String::from_utf8(buf2).unwrap();

    log::trace!("Message: {}", str);

    log::trace!(
        "{:#x}: [{}] [SYSCALL] send => {:#x}",
        unicorn.reg_read(RegisterARM::PC).unwrap(),
        unicorn.get_data().inner.thread_id(),
        res
    );

    res
}

pub fn getpeername(
    unicorn: &mut Unicorn<'_, Context>,
    socket_fd: u32,
    addr: u32,
    addr_len: u32,
) -> u32 {
    log::trace!(
        "{:#x}: [{}] [SYSCALL] getpeername(socket_fd = {:#x}, addr: {:#x}, addr_len: {:#x}) [IN]",
        unicorn.reg_read(RegisterARM::PC).unwrap(),
        unicorn.get_data().inner.thread_id(),
        socket_fd,
        addr,
        addr_len,
    );

    // TODO: return a valid emulated peer address
    let res = 0;

    log::trace!(
        "{:#x}: [{}] [SYSCALL] getpeername => {:#x}",
        unicorn.reg_read(RegisterARM::PC).unwrap(),
        unicorn.get_data().inner.thread_id(),
        res
    );

    res
}

pub fn listen(unicorn: &mut Unicorn<'_, Context>, socket_fd: u32, backlog: u32) -> u32 {
    log::trace!(
        "{:#x}: [{}] [SYSCALL] listen(socket_fd = {:#x}, backlog: {:#x}) [IN]",
        unicorn.reg_read(RegisterARM::PC).unwrap(),
        unicorn.get_data().inner.thread_id(),
        socket_fd,
        backlog,
    );
    let res = 0;
    log::trace!(
        "{:#x}: [{}] [SYSCALL] listen => {:#x}",
        unicorn.reg_read(RegisterARM::PC).unwrap(),
        unicorn.get_data().inner.thread_id(),
        res
    );
    res
}

pub fn accept(unicorn: &mut Unicorn<'_, Context>, socket_fd: u32, addr: u32, addr_len: u32) -> u32 {
    log::trace!(
        "{:#x}: [{}] [SYSCALL] accept(socket_fd = {:#x}, addr: {:#x}, addr_len: {:#x}) [IN]",
        unicorn.reg_read(RegisterARM::PC).unwrap(),
        unicorn.get_data().inner.thread_id(),
        socket_fd,
        addr,
        addr_len,
    );
    let res = block_for_receive(unicorn, socket_fd, 0);
    log::trace!(
        "{:#x}: [{}] [SYSCALL] accept => {:#x}",
        unicorn.reg_read(RegisterARM::PC).unwrap(),
        unicorn.get_data().inner.thread_id(),
        res
    );
    res
}

pub fn socketpair(
    unicorn: &mut Unicorn<'_, Context>,
    domain: u32,
    socket_type: u32,
    protocol: u32,
    sv_addr: u32,
) -> u32 {
    log::trace!(
        "{:#x}: [{}] [SYSCALL] socketpair(domain = {:#x}, socket_type: {:#x}, protocol: {:#x}, sv: {:#x}) [IN]",
        unicorn.reg_read(RegisterARM::PC).unwrap(),
        unicorn.get_data().inner.thread_id(),
        domain,
        socket_type,
        protocol,
        sv_addr,
    );

    let (a, b) = {
        let state = &mut unicorn.get_data().inner.sys_calls_state.lock().unwrap();
        let a = state.next_socket_fd;
        let b = a + 1;
        state.next_socket_fd = b + 1;
        state.socket_fds.insert(a);
        state.socket_fds.insert(b);
        (a, b)
    };
    unicorn.mem_write(sv_addr as u64, &a.to_le_bytes()).unwrap();
    unicorn
        .mem_write((sv_addr + 4) as u64, &b.to_le_bytes())
        .unwrap();

    let res = 0;
    log::trace!(
        "{:#x}: [{}] [SYSCALL] socketpair => {:#x}",
        unicorn.reg_read(RegisterARM::PC).unwrap(),
        unicorn.get_data().inner.thread_id(),
        res
    );
    res
}

pub fn sendto(
    unicorn: &mut Unicorn<'_, Context>,
    socket_fd: u32,
    buf: u32,
    len: u32,
    flags: u32,
    dest_addr: u32,
    addr_len: u32,
) -> u32 {
    log::trace!(
        "{:#x}: [{}] [SYSCALL] sendto(socket_fd = {:#x}, buf: {:#x}, len: {:#x}, flags: {:#x}, dest: {:#x}, addr_len: {:#x}) [IN]",
        unicorn.reg_read(RegisterARM::PC).unwrap(),
        unicorn.get_data().inner.thread_id(),
        socket_fd,
        buf,
        len,
        flags,
        dest_addr,
        addr_len,
    );
    let res = if len == 0 {
        0
    } else {
        send(unicorn, socket_fd, buf, len, flags)
    };
    log::trace!(
        "{:#x}: [{}] [SYSCALL] sendto => {:#x}",
        unicorn.reg_read(RegisterARM::PC).unwrap(),
        unicorn.get_data().inner.thread_id(),
        res
    );
    res
}

pub fn recv(
    unicorn: &mut Unicorn<'_, Context>,
    socket_fd: u32,
    buf: u32,
    len: u32,
    flags: u32,
) -> u32 {
    log::trace!(
        "{:#x}: [{}] [SYSCALL] recv(socket_fd = {:#x}, buf: {:#x}, len: {:#x}, flags: {:#x}) [IN]",
        unicorn.reg_read(RegisterARM::PC).unwrap(),
        unicorn.get_data().inner.thread_id(),
        socket_fd,
        buf,
        len,
        flags,
    );
    let res = block_for_receive(unicorn, socket_fd, flags);
    log::trace!(
        "{:#x}: [{}] [SYSCALL] recv => {:#x}",
        unicorn.reg_read(RegisterARM::PC).unwrap(),
        unicorn.get_data().inner.thread_id(),
        res
    );
    res
}

pub fn recvfrom(
    unicorn: &mut Unicorn<'_, Context>,
    socket_fd: u32,
    buf: u32,
    len: u32,
    flags: u32,
    src_addr: u32,
    addr_len: u32,
) -> u32 {
    log::trace!(
        "{:#x}: [{}] [SYSCALL] recvfrom(socket_fd = {:#x}, buf: {:#x}, len: {:#x}, flags: {:#x}, src: {:#x}, addr_len: {:#x}) [IN]",
        unicorn.reg_read(RegisterARM::PC).unwrap(),
        unicorn.get_data().inner.thread_id(),
        socket_fd,
        buf,
        len,
        flags,
        src_addr,
        addr_len,
    );
    let res = block_for_receive(unicorn, socket_fd, flags);
    log::trace!(
        "{:#x}: [{}] [SYSCALL] recvfrom => {:#x}",
        unicorn.reg_read(RegisterARM::PC).unwrap(),
        unicorn.get_data().inner.thread_id(),
        res
    );
    res
}

pub fn shutdown(unicorn: &mut Unicorn<'_, Context>, socket_fd: u32, how: u32) -> u32 {
    log::trace!(
        "{:#x}: [{}] [SYSCALL] shutdown(socket_fd = {:#x}, how: {:#x}) [IN]",
        unicorn.reg_read(RegisterARM::PC).unwrap(),
        unicorn.get_data().inner.thread_id(),
        socket_fd,
        how,
    );
    let res = 0;
    log::trace!(
        "{:#x}: [{}] [SYSCALL] shutdown => {:#x}",
        unicorn.reg_read(RegisterARM::PC).unwrap(),
        unicorn.get_data().inner.thread_id(),
        res
    );
    res
}

pub fn sendmsg(
    unicorn: &mut Unicorn<'_, Context>,
    socket_fd: u32,
    msg_addr: u32,
    flags: u32,
) -> u32 {
    log::trace!(
        "{:#x}: [{}] [SYSCALL] sendmsg(socket_fd = {:#x}, msg: {:#x}, flags: {:#x}) [IN]",
        unicorn.reg_read(RegisterARM::PC).unwrap(),
        unicorn.get_data().inner.thread_id(),
        socket_fd,
        msg_addr,
        flags,
    );
    let res = if is_emulated_socket(unicorn, socket_fd) {
        0
    } else {
        -9i32 as u32
    };
    log::trace!(
        "{:#x}: [{}] [SYSCALL] sendmsg => {:#x}",
        unicorn.reg_read(RegisterARM::PC).unwrap(),
        unicorn.get_data().inner.thread_id(),
        res
    );
    res
}

pub fn recvmsg(
    unicorn: &mut Unicorn<'_, Context>,
    socket_fd: u32,
    msg_addr: u32,
    flags: u32,
) -> u32 {
    log::trace!(
        "{:#x}: [{}] [SYSCALL] recvmsg(socket_fd = {:#x}, msg: {:#x}, flags: {:#x}) [IN]",
        unicorn.reg_read(RegisterARM::PC).unwrap(),
        unicorn.get_data().inner.thread_id(),
        socket_fd,
        msg_addr,
        flags,
    );

    // TODO: queue netlink/kernel events and wake this thread with a payload
    let res = block_for_receive(unicorn, socket_fd, flags);

    log::trace!(
        "{:#x}: [{}] [SYSCALL] recvmsg => {:#x}",
        unicorn.reg_read(RegisterARM::PC).unwrap(),
        unicorn.get_data().inner.thread_id(),
        res
    );

    res
}
