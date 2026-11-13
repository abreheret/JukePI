"""Probe: print every HEOS event with timestamps while a track plays (progress cadence, errors at end of track).
Serve clipA.mp3 (any short mp3) at http://<MUSIC_SERVER>/clipA.mp3 or change the URL.
Environment: DENON_HOST (Denon IP), DENON_PID (HEOS player id, see get_players), MUSIC_SERVER (host:port of the music server).
"""
import os
import socket,json,time,threading
T0=time.time(); pid=int(os.environ.get("DENON_PID","123456789"))
s=socket.create_connection((os.environ.get("DENON_HOST","192.168.1.60"),1255),timeout=60)
def log(m): print(f"[{time.time()-T0:7.2f}] {m}",flush=True)
def send(c): s.sendall((c+"\r\n").encode()); log(">> "+c.split('heos://')[1][:60])
def reader():
    buf=b""
    while True:
        try: d=s.recv(8192)
        except Exception as e: log(f"recv end {e}"); return
        if not d: return
        buf+=d
        while b"\r\n" in buf:
            l,buf=buf.split(b"\r\n",1)
            try: j=json.loads(l)
            except: log("raw "+l.decode(errors='ignore')); continue
            h=j.get("heos",{}); log(f"{h.get('command')} | {h.get('message','')[:90]}")
threading.Thread(target=reader,daemon=True).start()
send("heos://system/register_for_change_events?enable=on"); time.sleep(1)
send(f"heos://player/set_play_state?pid={pid}&state=stop"); time.sleep(1)
send(f"heos://browse/play_stream?pid={pid}&url=http://"+os.environ.get("MUSIC_SERVER","192.168.1.50:8765")+"/clipA.mp3")
time.sleep(32)
