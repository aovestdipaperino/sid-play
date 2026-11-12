"""Print the median autocorrelation peak (tonality) of a mono 16-bit WAV,
or with --rms-diff A B the RMS difference of two WAVs as a fraction of full scale."""
import array, sys, wave

if len(sys.argv) == 4 and sys.argv[1] == "--rms-diff":
    def load(p):
        w = wave.open(p); ch = w.getnchannels()
        a = array.array('h'); a.frombytes(w.readframes(w.getnframes()))
        return a[::ch]
    a, b = load(sys.argv[2]), load(sys.argv[3])
    n = min(len(a), len(b))
    rms = (sum((a[i] - b[i]) ** 2 for i in range(n)) / max(n, 1)) ** 0.5 / 32768
    print(f"{rms:.4f}")
    sys.exit(0)

def peak(seg, sr):
    n = len(seg); m = sum(seg) / n; seg = [v - m for v in seg]
    e = sum(v * v for v in seg)
    if e < 1e-9:
        return 0.0
    return max(sum(seg[i] * seg[i + lag] for i in range(0, n - lag, 2)) * 2 / e
               for lag in range(sr // 2000, sr // 60, 2))

w = wave.open(sys.argv[1]); sr = w.getframerate(); ch = w.getnchannels()
a = array.array('h'); a.frombytes(w.readframes(w.getnframes()))
x = list(a[::ch])[::4]; r = sr // 4
ps = sorted(peak(x[s:s + r // 10], r) for s in range(r, len(x) - r // 10, 2 * r))
print(f"{ps[len(ps) // 2]:.2f}")
