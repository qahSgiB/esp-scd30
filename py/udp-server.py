import socket

sock = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
sock.bind(("192.168.1.4", 9125))

while True:
    print('caka sa packet ...')

    (data, addr) = sock.recvfrom(1024)
    message = data.decode('utf-8')
    (ip, port) = addr

    print('  ==  dosiel novy packet  ==')
    print(f'ip : {ip}')
    print(f'port : {port}')
    print(message)
    print()
