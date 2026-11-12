"""Download upstream VoxLock, GTCRN and DPDFNet2 models locally."""
from pathlib import Path
import hashlib
import shutil
from urllib.request import urlopen
import voxlock

source = voxlock.download(precision="int8")
target = Path(__file__).resolve().parent / "models"
target.mkdir(exist_ok=True)
for name in ("enroll.onnx", "chunk_int8.onnx"):
    shutil.copy2(source / name, target / name)
    print(f"已安装 {name}")

base_url = 'https://github.com/k2-fsa/sherpa-onnx/releases/download/speech-enhancement-models/'
checksums = {
    'gtcrn_simple.onnx': 'e77603ac0c23dac3227dd2d7135b3a585cbee2679048aecfa886657d3ae1b534',
    'dpdfnet2.onnx': 'ce35d6025fc71df0ef10d1540e1b7916837bbfe5f6896deb744508d2cad487a9',
}
for name, expected in checksums.items():
    with urlopen(base_url + name, timeout=60) as response:
        data = response.read()
    if hashlib.sha256(data).hexdigest() != expected:
        raise RuntimeError(f'{name} 模型校验失败；未安装下载文件')
    temporary = target / (name + '.tmp')
    try:
        temporary.write_bytes(data)
        temporary.replace(target / name)
    finally:
        temporary.unlink(missing_ok=True)
    print(f'已安装 {name}')
