def compute_crc(v: int, poly: int, init: int) -> int:
    poly_degree = poly.bit_length() - 1
    v_bits = 8 if v == 0 else ((v.bit_length() - 1) // 8 + 1) * 8 # round to bytes

    # print(f'{poly_degree=}')
    # print(f'{v_bits=}')

    v = (v ^ (init << (v_bits - poly_degree))) << poly_degree
    poly = poly << (v_bits - 1)
    check_one = 1 << (poly_degree + v_bits - 1)

    for _ in range(v_bits):
        # print(f'v | {v:_>{v_bits + poly_degree}b}')
        # print(f'{check_one:_>{v_bits + poly_degree}b}')
        # print(f'p | {poly:_>{v_bits + poly_degree}b}')

        if v & check_one != 0:
            v ^= poly

        poly >>= 1
        check_one >>= 1

    # print(f'v | {v:_>{v_bits + poly_degree}b}')

    return v


sdc_poly = 0x131
sdc_init = 0xff


# precompute the table
crc_table_init = [compute_crc(v, sdc_poly, sdc_init) for v in range(256)]
crc_table = [compute_crc(v, sdc_poly, 0x00) for v in range(256)]

def get_crc_l1(v: int, init: bool):
    return crc_table_init[v] if init else crc_table[v]

# crc_table ^ 0xac = crc_table_init
def get_crc_l2(v2: int, v1: int):
    t = crc_table[v2] ^ 0xac ^ v1
    return crc_table[t]


for i in range(16):
    print('    ' + ', '.join(map(lambda x: f'0x{x:0>2x}', crc_table[(i * 16):((i + 1) * 16)])) + ',')


# print(f'{get_crc_l1(0xef ^ get_crc_l1(0xbe, True), False):#x}')


# t = compute_crc(0x0342, 0x131, 0xff)
# t = compute_crc(0xbeef, 0x131, 0xff)
# print(f'{t=}  (0x{t:X})')