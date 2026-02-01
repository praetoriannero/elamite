from dataclasses import dataclass
from enum import Enum
from pathlib import Path


class OptLevel(Enum):
    DEBUG = 0
    RELEASE = 1
    RELWITHDEBUGINFO = 2


@dataclass
class CompilerOptions:
    input_path: Path
    level: OptLevel = OptLevel.DEBUG
