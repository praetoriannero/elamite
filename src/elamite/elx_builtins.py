from elamite.elx_types import Type, Identifier

BUILTINS_MAP = {
    "u8": "uint8_t",
    "u16": "uint16_t",
    "u32": "uint32_t",
    "u64": "uint64_t",
    "i8": "int8_t",
    "i16": "int16_t",
    "i32": "int32_t",
    "i64": "int64_t",
    "f32": "float",
    "f64": "double",
    "bool": "bool",
    "str": "char*",
    "unit": "void",
}

BUILTINS = {Identifier(bi): Type(Identifier(bi)) for bi in BUILTINS_MAP.keys()}
