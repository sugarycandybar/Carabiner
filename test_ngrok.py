from carabiner.backend.ngrok_manager import NgrokManager
import time

m = NgrokManager()

def fake_read_output():
    last_error = ""
    for line in iter(m._process.stdout.readline, ""):
        print("LINE:", repr(line))
        if "lvl=crit" in line or "lvl=error" in line or "ERROR:" in line:
            if "err=" in line:
                last_error = line.split("err=")[1].strip().strip('"')
                print("FOUND ERR=:", last_error)
            else:
                last_error = line.replace("ERROR:", "").strip()
                print("FOUND ERROR::", last_error)

    m._process.wait()
    print("FINISHED. last_error=", repr(last_error))

m._read_output = fake_read_output
m.start(25565, "tcp")
time.sleep(2)
m.stop()
