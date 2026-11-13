"""Probe: does clear_queue during playback stop the track? Does play_stream emit a spurious stop? (see docs/HEOS-FINDINGS.md)
Needs the music server running on the Pi (stream urls) and the Denon reachable. Edit DENON/pid below.
Environment: DENON_HOST (Denon IP), DENON_PID (HEOS player id, see get_players), MUSIC_SERVER (host:port of the music server).
"""
import os
import socket,json,time,threading
pid=int(os.environ.get("DENON_PID","123456789")); T0=time.time()
s=socket.create_connection((os.environ.get("DENON_HOST","192.168.1.60"),1255),timeout=60)
def log(m): print(f"[{time.time()-T0:6.2f}] {m}",flush=True)
def send(c): s.sendall((c+"\r\n").encode()); log(">> "+c.split("heos://")[1][:70])
def rd():
    buf=b""
    while True:
        try: d=s.recv(8192)
        except Exception: return
        if not d: return
        buf+=d
        while b"\r\n" in buf:
            l,buf=buf.split(b"\r\n",1)
            try: h=json.loads(l)["heos"]
            except: continue
            c=h.get("command",""); 
            if c=="event/player_now_playing_progress": continue
            log(f"{c} | {h.get('result','')} | {h.get('message','')[:80]}")
threading.Thread(target=rd,daemon=True).start()
send("heos://system/register_for_change_events?enable=on"); time.sleep(1)
send(f"heos://player/set_play_state?pid={pid}&state=stop"); time.sleep(1)
send(f"heos://player/clear_queue?pid={pid}"); time.sleep(1)
B="http://"+os.environ.get("MUSIC_SERVER","192.168.1.50:8765")+"/stream/"
send(f"heos://browse/play_stream?pid={pid}&url={B}1768.mp3"); time.sleep(5)
log("=== clear_queue pendant lecture"); send(f"heos://player/clear_queue?pid={pid}"); time.sleep(4)
send(f"heos://player/get_play_state?pid={pid}"); time.sleep(1)
log("=== play_stream 2e titre pendant lecture/stop"); send(f"heos://browse/play_stream?pid={pid}&url={B}1769.mp3"); time.sleep(4)
send(f"heos://player/get_queue?pid={pid}"); time.sleep(1)
send(f"heos://player/set_play_state?pid={pid}&state=stop"); time.sleep(1.5)
