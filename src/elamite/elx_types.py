from __future__ import annotations
from dataclasses import dataclass
from lark.tree import Meta
from enum import Enum


SEP = "__"


class BinOp(Enum):
    OR = "||"
    AND = "&&"
    EQ = "=="
    NE = "!="
    GT = ">"
    LT = "<"
    GTE = ">="
    LTE = "<="
    BOR = "|"
    BXOR = "^"
    BAND = "&"
    SHL = "<<"
    SHR = ">>"
    ADD = "+"
    SUB = "-"
    MUL = "*"
    DIV = "/"
    MOD = "%"


class AsOp(Enum):
    ADDEQ = "+="
    SUBEQ = "-="
    MULEQ = "*="
    DIVEQ = "/="
    MODEQ = "%="
    SHLEQ = "<<="
    SHREQ = ">>="
    ANDEQ = "&="
    OREQ = "|="
    XOREQ = "^="
    EQ = "="


class UnOp(Enum):
    POS = "+"
    NEG = "-"
    BNOT = "~"
    NOT = "!"
    REF = "&"
    DER = "*"


type Expr = Identifier | Integer | Float | Bool | String | FuncCall | BinaryOp | UnaryOp


type PostFix = GetAttr | GetSlice | FuncCall


type Item = FuncDef | StructDef | GlobalDef | EnumDef | ModuleDef


@dataclass(unsafe_hash=True)
class Symbol:
    """
    Parent class for defining symbols, top level items in a module.
    """

    ident: str


@dataclass(unsafe_hash=True)
class Module(Symbol):
    symbols: dict


@dataclass
class StructDef(Symbol):
    fields: list


@dataclass
class EnumDef(Symbol):
    variants: list


@dataclass
class FuncDef(Symbol):
    params: list[FuncParam]
    ret_type: Type | None
    block: list[Stmt]


@dataclass
class GlobalDef(Symbol):
    item_type: str
    type: str
    expr: str


@dataclass
class FuncParam:
    ident: str
    type: str


@dataclass
class StructField:
    ident: str
    type: str


@dataclass
class Stmt:
    meta: Meta


@dataclass
class AssignStmt(Stmt):
    op: AssignOp
    lhs: Identifier
    rhs: Expr


@dataclass
class LetStmt(Stmt):
    ident: Identifier
    type: Type | None
    value: str
    mut: bool


@dataclass
class ReturnStmt(Stmt):
    expr: Expr | None


@dataclass
class ExprStmt(Stmt):
    expr: Expr


@dataclass
class ForStmt(Stmt):
    ident: Identifier
    iterator: Expr
    block: list[Stmt]


@dataclass
class WhileStmt(Stmt):
    expr: Expr
    block: list[Stmt]


@dataclass
class IfStmt(Stmt):
    expr: Expr
    block: list[Stmt]
    elif_clauses: list[ElifClause]
    else_clause: ElseClause | None


@dataclass
class ElifClause(Stmt):
    expr: Expr
    block: list[Stmt]


@dataclass
class ElseClause(Stmt):
    block: list[Stmt]


@dataclass
class BreakStmt(Stmt):
    expr: Expr | None


@dataclass
class ContinueStmt(Stmt):
    expr: Expr | None


@dataclass
class ModuleStmt(Stmt):
    ident: Identifier
    block: list[Stmt]


@dataclass
class BinaryOp:
    op: BinOp
    lhs: Expr
    rhs: Expr


@dataclass
class UnaryOp:
    op: UnOp
    inner: Expr


@dataclass
class AssignOp:
    op: AsOp
    lhs: Expr
    rhs: Expr


@dataclass
class Type:
    ident: Identifier


@dataclass(frozen=True)
class Identifier:
    name: str


@dataclass
class Integer:
    value: str


@dataclass
class Float:
    value: str


@dataclass
class Bool:
    value: bool


@dataclass
class Unit:
    pass


@dataclass
class Array:
    size: Expr
    type: Type


@dataclass
class ModuleDef:
    ident: Identifier
    block: list[Stmt]


@dataclass
class String:
    value: str


@dataclass
class PostfixExpr:
    ident: Identifier
    postfixes: list[PostFix]


@dataclass
class PrefixExpr:
    prefixes: list[Prefix]
    ident: Identifier


class Prefix(Enum):
    PLUS = "+"
    NEG = "-"
    BINV = "~"
    NOT = "!"
    REF = "&"
    DEREF = "*"


@dataclass
class FuncCall:
    args: list[Expr]


@dataclass
class GetAttr:
    ident: Identifier


@dataclass
class RangeStart:
    value: Expr


@dataclass
class RangeInclusive:
    is_incl: bool


@dataclass
class RangeEnd:
    value: Expr


@dataclass
class Range:
    start: RangeStart | None
    end: RangeEnd | None
    inclusive: RangeInclusive | None


@dataclass
class GetSlice:
    slice: Range | Expr
