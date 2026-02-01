from abc import ABC
from dataclasses import dataclass


import elamite.elx_types as et


class CItem(ABC):
    def emit(self) -> str:
        raise NotImplementedError()


@dataclass
class CStructDef(CItem):
    struct_def: et.StructDef


@dataclass
class CFuncDef(CItem):
    func_def: et.FuncDef


@dataclass
class CGlobalDef(CItem):
    global_def: et.GlobalDef


@dataclass
class CEnumDef(CItem):
    enum_def: et.EnumDef


@dataclass
class CModuleStmt(CItem):
    mod_stmt: et.ModuleStmt


@dataclass
class CImportModule(CItem):
    import_stmt: et.ImportStmt

