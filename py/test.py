import serial

ser = serial.Serial()
ser.port = 'COM3'
ser.baudrate = 9600
ser.dtr = False
ser.rts = False

ser.open()

ser.dtr = False
ser.rts = False

def run():
    while True:
        b = ser.readline()
        print(b)