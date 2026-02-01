from dataclasses import dataclass
from enum import Enum
from pathlib import Path


class ProjectType(Enum):
    BIN = 0
    LIB = 1


class OptLevel(Enum):
    DEBUG = 0
    RELEASE = 1
    RELWITHDEBUGINFO = 2


@dataclass
class CompilerOptions:
    input_path: Path
    project_type: ProjectType = ProjectType.BIN
    level: OptLevel = OptLevel.DEBUG
