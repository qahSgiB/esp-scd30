import socket

esp_address = ("192.168.1.8", 9123)
sock = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)

sock.sendto(b'dobre ranko z python vysielaca', esp_address)

while True:
    print('enter a message')
    message = input()
    if len(message) == 0:
        break

    sock.sendto(message.encode('utf-8'), esp_address)

sock.sendto(b'dovidenia z python vysielaca', esp_address)
