"""End-to-end player test against the real server + Denon: play 3 short tracks, pause/resume/next, natural chaining.
Pick track ids with: sqlite3 ~/.local/share/music-server/index.db "select id,title,duration_ms from tracks where duration_ms between 8000 and 25000 limit 10"
Run: python3 scripts/test_player.py   (edit the ids below). The Denon will make noise.
Environment: DENON_HOST (Denon IP), DENON_PID (HEOS player id, see get_players), MUSIC_SERVER (host:port of the music server).
"""
import os
import json,time,urllib.request as u
B="http://"+os.environ.get("MUSIC_SERVER","192.168.1.50:8765"); T0=time.time()
def call(path,body=None):
    r=u.Request(B+path,data=json.dumps(body).encode() if body is not None else (b"" if path!="/player" else None),
                headers={"Content-Type":"application/json"},method="GET" if path=="/player" else "POST")
    return json.load(u.urlopen(r,timeout=10))
def st(): return call("/player")
def show(tag,s): print(f"[{time.time()-T0:5.1f}] {tag:22} status={s['status']:8} idx={s['index']} pos={s['position_ms']/1000:5.1f}/{s['duration_ms']/1000:.1f} err={s['error']}",flush=True)
s=call("/player/play",{"track_ids":[36062,35376,26005]}); show("play queue(3)",s)
last=None; actions={6:"pause",9:"resume",12:"next"}; done=set()
while time.time()-T0<75:
    s=st(); key=(s['status'],s['index'])
    if key!=last: show("change",s); last=key
    t=int(time.time()-T0)
    for at,a in actions.items():
        if t>=at and a not in done:
            done.add(a); show(f"-> {a}",s); call("/player/"+a)
    if s['status']=='stopped' and time.time()-T0>20: break
    time.sleep(0.4)
show("final",st())
print("volume:",st()['volume'])
