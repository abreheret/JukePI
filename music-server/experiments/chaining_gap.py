"""Measure the silence between two tracks: naive (send next after stop) vs pre-emptive (send next N s before the end).
Needs two ~20 s clips reachable at BASE (clipA.mp3, clipB.mp3). Results: docs/HEOS-FINDINGS.md section 4.
Environment: DENON_HOST (Denon IP), DENON_PID (HEOS player id, see get_players), MUSIC_SERVER (host:port of the music server).
"""
import os
import socket,json,time,threading,re
pid=int(os.environ.get("DENON_PID","123456789")); BASE="http://"+os.environ.get("MUSIC_SERVER","192.168.1.50:8765")+"/"
s=socket.create_connection((os.environ.get("DENON_HOST","192.168.1.60"),1255),timeout=60)
ev=[]; T0=time.time()
def now(): return time.time()-T0
def send(c): s.sendall((c+"\r\n").encode())
def reader():
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
            ev.append((now(),h.get("command"),h.get("message","")))
threading.Thread(target=reader,daemon=True).start()
def wait(pred,start,timeout):
    t=time.time()
    while time.time()-t<timeout:
        for i in range(start,len(ev)):
            if pred(ev[i]): return i,ev[i]
        time.sleep(0.01)
    return None,None
def st(state): return lambda e: e[1]=="event/player_state_changed" and f"state={state}" in e[2]
def prog(e): return e[1]=="event/player_now_playing_progress"
def errs(a): return [e for e in ev[a:] if "error" in e[1]]
def reset():
    send(f"heos://player/set_play_state?pid={pid}&state=stop"); time.sleep(1)
    send(f"heos://player/clear_queue?pid={pid}"); time.sleep(1.5)
send("heos://system/register_for_change_events?enable=on"); time.sleep(1)
def start_A():
    reset(); i0=len(ev)
    send(f"heos://browse/play_stream?pid={pid}&url={BASE}clipA.mp3")
    i,e=wait(prog,i0,10); 
    # A end estimate: progress event time + remaining
    m=re.search(r"cur_pos=(\d+)&duration=(\d+)",e[2]); cur,dur=int(m[1]),int(m[2])
    return i0,e[0]+(dur-cur)/1000
res={}
# V1: naive, send B when Denon reports stop after A
i0,endA=start_A()
j,e=wait(lambda e: e[1]=="event/player_state_changed" and "state=stop" in e[2] and e[0]>endA-3,i0,40)
tstop=e[0] if e else None; ib=len(ev)
send(f"heos://browse/play_stream?pid={pid}&url={BASE}clipB.mp3")
k,eb=wait(st("play"),ib,10)
res["V1 naive (after stop event)"]=dict(A_end=round(endA,2),stop_event=round(tstop,2) if tstop else None,B_play=round(eb[0],2) if eb else None,
    gap_s=round(eb[0]-endA,2) if eb else None,errors=len(errs(i0)))
# V2: pre-emptive, send B `lead` s before A ends
for lead in (0.55,1.0):
    i0,endA=start_A()
    while now()<endA-lead: time.sleep(0.005)
    ib=len(ev); ts=now()
    send(f"heos://browse/play_stream?pid={pid}&url={BASE}clipB.mp3")
    k,eb=wait(st("play"),ib,10)
    res[f"V2 pre-emptive lead={lead}s"]=dict(A_end=round(endA,2),sent=round(ts,2),B_play=round(eb[0],2) if eb else None,
        gap_s=round(eb[0]-endA,2) if eb else None,errors=len(errs(i0)))
    time.sleep(2)
for k,v in res.items(): print(k,v)
reset()
