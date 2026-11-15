param(
    [Parameter(Mandatory=$true)][string]$Python,
    [string]$Destination = (Join-Path $env:TEMP 'voice-isolation-cpu-benchmark')
)
$ErrorActionPreference = 'Stop'
New-Item -ItemType Directory -Force $Destination | Out-Null
$speech = New-Object -ComObject SAPI.SpVoice
$stream = New-Object -ComObject SAPI.SpFileStream
$stream.Open((Join-Path $Destination 'speech.wav'), 3, $false)
try {
    $speech.AudioOutputStream = $stream
    $utterance = 'Today we are testing a voice isolation system with a synthetic speech sample. The quick brown fox jumps over the lazy dog. Good morning, please check the microphone and adjust the volume. We are listening to the sound of a computer voice in a quiet room. The words should remain clear and natural during this test. '
    $speech.Speak(($utterance * 4)) | Out-Null
} finally { $stream.Close() }
$converter = @'
import sys
import numpy as np
from scipy.io import wavfile
from scipy.signal import resample_poly
source, output, reference = sys.argv[1:]
rate, audio = wavfile.read(source)
if rate != 22050 or audio.ndim != 1 or audio.dtype != np.int16:
    raise ValueError('SAPI voice must produce mono 16-bit 22050 Hz WAV')
samples = np.clip(np.rint(resample_poly(audio.astype(np.float32), 320, 147)), -32768, 32767).astype(np.int16)
if len(samples) < 32 * 48000: raise ValueError('Synthetic speech is shorter than 32 seconds')
wavfile.write(output, 48000, samples[:32*48000])
wavfile.write(reference, 48000, samples[:6*48000])
'@
$converter | & $Python - (Join-Path $Destination 'speech.wav') (Join-Path $Destination 'input.wav') (Join-Path $Destination 'reference.wav')
if ($LASTEXITCODE -ne 0) { throw 'Audio conversion failed' }
Write-Host "Test audio written to $Destination"
