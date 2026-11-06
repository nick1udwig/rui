"""Bounded WebSocket trace replay for the authored source-aware demo."""
import base64
import hashlib
import socket
import struct
import time

def fields(data):
    at=0;result={}
    def varint():
        nonlocal at
        value=0
        for shift in range(0,70,7):
            if at>=len(data):raise ValueError('truncated varint')
            byte=data[at];at+=1;value|=(byte&127)<<shift
            if not byte&128:return value
        raise ValueError('varint limit')
    while at<len(data):
        key=varint();tag=key>>3;wire=key&7
        if wire==0:result[tag]=varint()
        elif wire==2:
            size=varint()
            if at+size>len(data):raise ValueError('truncated field')
            result[tag]=data[at:at+size];at+=size
        else:raise ValueError('unsupported fixture field')
    return result

def exact(connection,size):
    out=bytearray()
    while len(out)<size:
        chunk=connection.recv(size-len(out))
        if not chunk:raise EOFError()
        out.extend(chunk)
    return out

def receive(connection):
    a,b=exact(connection,2)
    if a==0x88:raise EOFError()
    if a!=0x82 or not b&128:raise ValueError('expected masked binary message')
    size=b&127
    if size==126:size=struct.unpack('>H',exact(connection,2))[0]
    elif size==127:size=struct.unpack('>Q',exact(connection,8))[0]
    if size>16384:raise ValueError('client packet limit')
    mask=exact(connection,4);data=exact(connection,size)
    return bytes(byte^mask[i%4] for i,byte in enumerate(data))

def send(connection,data):
    size=len(data)
    header=bytes([0x82,size]) if size<126 else b'\x82\x7e'+struct.pack('>H',size) if size<=65535 else b'\x82\x7f'+struct.pack('>Q',size)
    connection.sendall(header+data)

def packets(path):
    with path.open('rb') as source:
        while header:=source.read(4):
            if len(header)!=4:raise ValueError('truncated length')
            size=struct.unpack('>I',header)[0]
            if not 0<size<=1024*1024:raise ValueError('server packet limit')
            packet=source.read(size)
            if len(packet)!=size:raise ValueError('truncated packet')
            yield packet

def serve(handler,path):
    host=handler.headers.get('Host','')
    if handler.headers.get('Origin') not in ['http://'+host,'https://'+host] or handler.headers.get('Sec-WebSocket-Protocol')!='rui.v1' or handler.headers.get('Upgrade','').lower()!='websocket':
        handler.send_error(403);return
    try:
        key=base64.b64decode(handler.headers.get('Sec-WebSocket-Key',''),validate=True)
        if len(key)!=16:raise ValueError('key')
    except ValueError:handler.send_error(400);return
    accept=base64.b64encode(hashlib.sha1((handler.headers['Sec-WebSocket-Key']+'258EAFA5-E914-47DA-95CA-C5AB0DC85B11').encode()).digest()).decode()
    connection=handler.connection;connection.settimeout(60);connection.setsockopt(socket.IPPROTO_TCP,socket.TCP_NODELAY,1)
    handler.send_response(101);handler.send_header('Upgrade','websocket');handler.send_header('Connection','Upgrade');handler.send_header('Sec-WebSocket-Accept',accept);handler.send_header('Sec-WebSocket-Protocol','rui.v1');handler.end_headers();handler.wfile.flush()
    try:
        if fields(receive(connection)).get(5)!=1:raise ValueError('expected client hello')
        due=time.monotonic();expected=None
        for packet in packets(path):
            kind=fields(packet)
            if kind[5] in (10,11):
                time.sleep(max(0,due-time.monotonic()));due=time.monotonic()+.04
                expected=fields(kind[6])[3]
            send(connection,packet)
            if kind[5]==13:
                while True:
                    ack=fields(receive(connection))
                    if ack.get(5)==31:
                        if fields(ack[6]).get(1)!=expected:raise ValueError('wrong presented revision')
                        break
        connection.sendall(b'\x88\x02\x03\xe8')
    except (OSError,ValueError,EOFError):pass
    handler.close_connection=True
