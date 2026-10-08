# Opens N keep-alive connections, sends one request on each, keeps them open.
# usage: hold.py <n> [port]
import socket, sys, time
n = int(sys.argv[1])
port = int(sys.argv[2]) if len(sys.argv) > 2 else 3000
socks = []
for _ in range(n):
    s = socket.create_connection(("127.0.0.1", port))
    s.sendall(b"GET /r0/1 HTTP/1.1\r\nHost: x\r\n\r\n")
    s.recv(4096)
    socks.append(s)
print("holding", n, flush=True)
time.sleep(3600)
