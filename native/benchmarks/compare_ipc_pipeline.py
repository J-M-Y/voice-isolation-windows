"""Compare synthetic 48k full-pipeline CPU vs DirectML-IPC output WAVs.

Signals are from prepare_gpu_fixture.py; official Rust and Python+worker tests
both warm 2.4 seconds and process the same following sample interval. Alignment
is by target cross-correlation after the first two measured seconds. Resulting
lag is an offline signal offset: it does not include capture buffering,
compute time, playback, device latency, or GPU-driver service overhead.
"""
import argparse
import hashlib
import json
from pathlib import Path
import numpy as np
from scipy.io import wavfile
from scipy.signal import correlate, correlation_lags

RATE=48000
WARM=round(2.4*RATE)


def read(path):
    rate,x=wavfile.read(path)
    if rate!=RATE or x.ndim!=1: raise ValueError(f'Expected mono 48kHz WAV: {path}')
    if x.dtype==np.int16: x=x.astype(np.float64)/32768
    elif x.dtype==np.float32: x=x.astype(np.float64)
    else: raise ValueError(f'Unsupported WAV sample type: {path}')
    if not np.isfinite(x).all(): raise ValueError(f'Nonfinite WAV samples: {path}')
    return x


def cosine(x,y):
    den=np.linalg.norm(x)*np.linalg.norm(y)
    if den<1e-15: raise ValueError('Cannot correlate silent signal')
    return float(np.dot(x,y)/den)


def measure(target, other, output):
    corr=correlate(output,target,mode='full',method='fft')
    lags=correlation_lags(len(output),len(target))
    ok=(lags>=0)&(lags<=3*RATE)
    lag=int(lags[ok][np.argmax(np.abs(corr[ok]))])
    start,stop=2*RATE,min(len(target),len(output)-lag)
    a,b,z=target[start:stop],other[start:stop],output[start+lag:stop+lag]
    if len(z)==0: raise ValueError('No aligned output remains')
    gain=np.linalg.lstsq(np.stack([a,b],axis=1),z,rcond=None)[0]
    return dict(lag_samples=lag,signal_lag_ms=lag*1000/RATE,target_correlation=cosine(a,z),
                interference_correlation=cosine(b,z), target_linear_gain=float(gain[0]),
                interference_linear_gain=float(gain[1]),output_rms=float(np.sqrt(np.mean(z*z))),
                aligned_seconds=len(z)/RATE)


def main():
    p=argparse.ArgumentParser(description=__doc__)
    for name in ('target','interference','mixture','cpu','gpu','output-dir'):
        p.add_argument('--'+name,type=Path,required=True)
    p.add_argument('--gpu-parent',choices=('python-simulation','rust'),default='python-simulation')
    a=p.parse_args()
    cpu,gpu=read(a.cpu),read(a.gpu)
    count=len(cpu)
    if count != len(gpu) or count <= 2*RATE: raise ValueError('CPU and GPU outputs must have equal length exceeding 2 seconds')
    target,other,mixture=(read(v)[WARM:WARM+count] for v in (a.target,a.interference,a.mixture))
    if not all(len(x)==count for x in (target,other,mixture)): raise ValueError('Fixture is too short after warmup')
    ca,ga=measure(target,other,cpu),measure(target,other,gpu)
    start=2*RATE;stop=min(len(target),len(cpu)-ca['lag_samples'],len(gpu)-ga['lag_samples'])
    c=cpu[start+ca['lag_samples']:stop+ca['lag_samples']]
    g=gpu[start+ga['lag_samples']:stop+ga['lag_samples']]
    d=c-g
    a.output_dir.mkdir(parents=True,exist_ok=True)
    count=min(12*RATE,stop-start)
    clips={'01-synthetic-mix.wav':mixture[start:start+count],
           '02-cpu-reference.wav':cpu[start+ca['lag_samples']:start+ca['lag_samples']+count],
           '03-gpu-experimental.wav':gpu[start+ga['lag_samples']:start+ga['lag_samples']+count]}
    peak=max(float(np.max(np.abs(v))) for v in clips.values())
    gain=0.90/peak if peak else 1.0
    for name,x in clips.items():
        wavfile.write(a.output_dir/name,RATE,np.clip(np.rint(x*gain*32767),-32768,32767).astype(np.int16))
    report=dict(cpu=ca,gpu=ga,cpu_gpu_correlation=cosine(c,g),
                cpu_gpu_rms_difference=float(np.sqrt(np.mean(d*d))),
                cpu_gpu_relative_rms_difference=float(np.sqrt(np.mean(d*d))/np.sqrt(np.mean(c*c))),
                preview_common_gain=gain,preview_seconds=count/RATE,
                preview_sha256={name:hashlib.sha256((a.output_dir/name).read_bytes()).hexdigest() for name in clips},
                scope=f'one synthetic mixture; no physical audio device; GPU parent: {a.gpu_parent}')
    (a.output_dir/'quality.json').write_text(json.dumps(report,ensure_ascii=False,indent=2),encoding='utf-8')
    print(json.dumps(report,ensure_ascii=False),flush=True)

if __name__=='__main__': main()
