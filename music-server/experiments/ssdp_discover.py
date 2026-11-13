"""Find the Denon on the LAN with SSDP (same search the Rust server does).
Environment: DENON_HOST (Denon IP), DENON_PID (HEOS player id, see get_players), MUSIC_SERVER (host:port of the music server).
"""
import socket,time
m=("M-SEARCH * HTTP/1.1\r\nHOST: 239.255.255.250:1900\r\nMAN: \"ssdp:discover\"\r\nMX: 3\r\nST: urn:schemas-denon-com:device:ACT-Denon:1\r\n\r\n").encode()
s=socket.socket(socket.AF_INET,socket.SOCK_DGRAM);s.settimeout(1)
s.sendto(m,("239.255.255.250",1900))
t=time.time()
while time.time()-t<5:
    try:
        d,a=s.recvfrom(2048);print(a[0]);print(d.decode(errors="ignore")[:300])
    except socket.timeout: pass
