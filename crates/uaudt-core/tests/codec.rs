use uaudt_core::{Builtin, Error, ErrorKind, FieldType, TypeDictionary, Value};

const RECIPE_XML: &[u8] = include_bytes!("fixtures/recipe.bsd");
/// Encoded by the original Python implementation (`StructuredTypeEncoder.encode_by_name`).
const RECIPE_BIN: &[u8] = include_bytes!("fixtures/recipe.bin");

fn dict() -> TypeDictionary {
    TypeDictionary::parse(RECIPE_XML).unwrap()
}

#[test]
fn parses_schema() {
    let d = dict();
    let recipe = d.struct_type(d.require_struct("Recipe").unwrap());
    let names: Vec<_> = recipe.fields.iter().map(|f| &*f.name).collect();
    // Length fields (NoOfX) are folded into the arrays.
    assert_eq!(
        names,
        [
            "Name", "Enabled", "Count", "Mode", "Created", "SubItems", "Origin", "Tags"
        ]
    );
    assert!(recipe.fields[5].is_array);
    // Forward reference to Point resolved.
    assert!(matches!(recipe.fields[6].ty, FieldType::Struct(_)));
    assert_eq!(recipe.fields[1].ty, FieldType::Builtin(Builtin::Boolean));
    assert_eq!(
        d.struct_type(d.require_struct("Point").unwrap())
            .fixed_size(),
        Some(8)
    );
    assert_eq!(recipe.fixed_size(), None);
    let mode = d.enum_type(d.enum_id("Mode").unwrap());
    assert_eq!(mode.value_of("Manual"), Some(2));
    assert_eq!(mode.name_of(1), Some("Auto"));
    // Prefixed lookups work too.
    assert!(d.struct_id("tns:Recipe").is_some());
}

#[test]
fn decodes_python_golden_bytes() {
    let d = dict();
    let id = d.require_struct("Recipe").unwrap();
    let (v, used) = d.decode_with_len(id, RECIPE_BIN).unwrap();
    assert_eq!(used, RECIPE_BIN.len());

    assert_eq!(d.get_str(&v, "Name").unwrap(), &Value::from("Recette-1"));
    assert_eq!(d.get_str(&v, "Enabled").unwrap(), &Value::Boolean(true));
    assert_eq!(d.get_str(&v, "Count").unwrap(), &Value::Int32(42));
    assert_eq!(
        d.get_str(&v, "Created").unwrap(),
        &Value::DateTime(133_000_000_000_000_000)
    );
    assert_eq!(
        d.get_str(&v, "SubItems[2].Description").unwrap(),
        &Value::from("Sub 2 é")
    );
    assert_eq!(
        d.get_str(&v, "SubItems[1].Value").unwrap(),
        &Value::Float(0.5)
    );
    assert_eq!(d.get_str(&v, "Origin.Y").unwrap(), &Value::Float(-2.25));
    assert_eq!(d.get_str(&v, "Tags[1]").unwrap(), &Value::from("é"));
    let mode = d.get_str(&v, "Mode").unwrap();
    assert_eq!(mode.display(&d).to_string(), "Manual");
}

#[test]
fn roundtrip_is_byte_exact() {
    let d = dict();
    let id = d.require_struct("Recipe").unwrap();
    let v = d.decode(id, RECIPE_BIN).unwrap();
    assert_eq!(d.encode(&v).unwrap(), RECIPE_BIN);
}

#[test]
fn byte_patch_matches_full_reencode() {
    let d = dict();
    let id = d.require_struct("Recipe").unwrap();
    let mods: Vec<(&str, Value)> = vec![
        ("Name", "Une recette plus longue".into()),
        ("SubItems[1].Description", "Modifié".into()),
        ("Origin.X", 10.into()), // Int32 coerced to Float
        ("Mode", "Auto".into()), // enum by name
        (
            "Tags",
            Value::Array(vec!["x".into(), "y".into(), "z".into()]),
        ),
    ];
    let fast = d.patch_str(id, RECIPE_BIN, &mods).unwrap();

    let mut v = d.decode(id, RECIPE_BIN).unwrap();
    for (p, val) in &mods {
        d.set_str(&mut v, p, val.clone()).unwrap();
    }
    assert_eq!(fast, d.encode(&v).unwrap());
    assert_eq!(
        d.get_str(&d.decode(id, &fast).unwrap(), "Origin.X")
            .unwrap(),
        &Value::Float(10.0)
    );
}

#[test]
fn overlapping_patch_falls_back() {
    let d = dict();
    let id = d.require_struct("Recipe").unwrap();
    let point = d.require_struct("Point").unwrap();
    let mut origin = d.default_struct(point).unwrap();
    d.set_str(&mut origin, "Y", 3.0f32.into()).unwrap();
    let out = d
        .patch_str(
            id,
            RECIPE_BIN,
            &[("Origin", origin.into()), ("Origin.X", 7.0f32.into())],
        )
        .unwrap();
    let v = d.decode(id, &out).unwrap();
    assert_eq!(d.get_str(&v, "Origin.X").unwrap(), &Value::Float(7.0));
    assert_eq!(d.get_str(&v, "Origin.Y").unwrap(), &Value::Float(3.0));
}

#[test]
fn read_path_decodes_only_target() {
    let d = dict();
    let id = d.require_struct("Recipe").unwrap();
    let p = d.compile_path(id, "SubItems[2].Id").unwrap();
    assert_eq!(d.read_path(RECIPE_BIN, &p).unwrap(), Value::Int32(2));
    let p = d.compile_path(id, "Tags").unwrap();
    assert_eq!(
        d.read_path(RECIPE_BIN, &p).unwrap(),
        Value::Array(vec!["a".into(), "é".into()])
    );
}

#[test]
fn default_struct_encodes() {
    let d = dict();
    let id = d.require_struct("Recipe").unwrap();
    let v = d.default_struct(id).unwrap();
    let bytes = d.encode(&v).unwrap();
    assert_eq!(d.decode(id, &bytes).unwrap(), v);
}

#[test]
fn path_errors() {
    let d = dict();
    let id = d.require_struct("Recipe").unwrap();
    assert!(matches!(
        (d.compile_path(id, "Nope")).map_err(Error::into_kind),
        Err(ErrorKind::InvalidPath { .. })
    ));
    assert!(matches!(
        (d.compile_path(id, "Name[0]")).map_err(Error::into_kind),
        Err(ErrorKind::InvalidPath { .. })
    ));
    assert!(matches!(
        (d.compile_path(id, "Count.X")).map_err(Error::into_kind),
        Err(ErrorKind::InvalidPath { .. })
    ));
    assert!(matches!(
        (d.compile_path(id, "SubItems[x]")).map_err(Error::into_kind),
        Err(ErrorKind::InvalidPath { .. })
    ));
    let p = d.compile_path(id, "SubItems[9].Id").unwrap();
    assert!(matches!(
        (d.read_path(RECIPE_BIN, &p)).map_err(Error::into_kind),
        Err(ErrorKind::IndexOutOfRange { .. })
    ));
}

#[test]
fn type_errors() {
    let d = dict();
    let id = d.require_struct("Recipe").unwrap();
    let mut v = d.default_struct(id).unwrap();
    assert!(matches!(
        (d.set_str(&mut v, "Count", "abc".into())).map_err(Error::into_kind),
        Err(ErrorKind::TypeMismatch { .. })
    ));
    assert!(matches!(
        (d.set_str(&mut v, "Mode", "Turbo".into())).map_err(Error::into_kind),
        Err(ErrorKind::InvalidEnum { .. })
    ));
    assert!(matches!(
        (d.set_str(&mut v, "Count", Value::Int64(1 << 40))).map_err(Error::into_kind),
        Err(ErrorKind::TypeMismatch { .. })
    ));
    // Strict encoder: a wrongly typed field is reported with its location.
    v.fields[2] = Value::Float(1.0);
    match d.encode(&v).map_err(Error::into_kind) {
        Err(ErrorKind::TypeMismatch { path, .. }) => assert_eq!(path, "Recipe.Count"),
        other => panic!("{other:?}"),
    }
}

#[test]
fn truncated_and_corrupt_input() {
    let d = dict();
    let id = d.require_struct("Recipe").unwrap();
    for cut in [0, 3, 10, 50, RECIPE_BIN.len() - 1] {
        assert!(
            matches!(
                (d.decode(id, &RECIPE_BIN[..cut])).map_err(Error::into_kind),
                Err(ErrorKind::UnexpectedEof { .. })
            ),
            "cut={cut}"
        );
    }
    let mut bad = RECIPE_BIN.to_vec();
    bad[0..4].copy_from_slice(&(-5i32).to_le_bytes());
    assert!(matches!(
        (d.decode(id, &bad)).map_err(Error::into_kind),
        Err(ErrorKind::InvalidLength { .. })
    ));
    // Huge array length must not allocate gigabytes.
    let mut huge = RECIPE_BIN.to_vec();
    huge[30..34].copy_from_slice(&i32::MAX.to_le_bytes());
    assert!(d.decode(id, &huge).is_err());
}

const ALL_TYPES_XML: &str = r#"<opc:TypeDictionary xmlns:opc="http://opcfoundation.org/BinarySchema/" xmlns:tns="urn:t" xmlns:ua="http://opcfoundation.org/UA/">
  <opc:EnumeratedType Name="Small" LengthInBits="8">
    <opc:EnumeratedValue Name="A" Value="0"/><opc:EnumeratedValue Name="B" Value="1"/>
  </opc:EnumeratedType>
  <opc:StructuredType Name="All">
    <opc:Field Name="B" TypeName="opc:Boolean"/>
    <opc:Field Name="Sb" TypeName="opc:SByte"/>
    <opc:Field Name="U8" TypeName="opc:Byte"/>
    <opc:Field Name="I16" TypeName="opc:Int16"/>
    <opc:Field Name="U16" TypeName="opc:UInt16"/>
    <opc:Field Name="I32" TypeName="opc:Int32"/>
    <opc:Field Name="U32" TypeName="opc:UInt32"/>
    <opc:Field Name="I64" TypeName="opc:Int64"/>
    <opc:Field Name="U64" TypeName="opc:UInt64"/>
    <opc:Field Name="F" TypeName="opc:Float"/>
    <opc:Field Name="D" TypeName="opc:Double"/>
    <opc:Field Name="S" TypeName="opc:String"/>
    <opc:Field Name="Dt" TypeName="opc:DateTime"/>
    <opc:Field Name="G" TypeName="opc:Guid"/>
    <opc:Field Name="Bs" TypeName="opc:ByteString"/>
    <opc:Field Name="Sc" TypeName="opc:StatusCode"/>
    <opc:Field Name="E" TypeName="tns:Small"/>
    <opc:Field Name="Sh" TypeName="opc:short"/>
  </opc:StructuredType>
  <opc:StructuredType Name="WithUnknown">
    <opc:Field Name="Ok" TypeName="opc:Int32"/>
    <opc:Field Name="Node" TypeName="ua:NodeId"/>
  </opc:StructuredType>
</opc:TypeDictionary>"#;

#[test]
fn all_builtins_roundtrip() {
    let d = TypeDictionary::parse(ALL_TYPES_XML.as_bytes()).unwrap();
    let id = d.require_struct("All").unwrap();
    let mut v = d.default_struct(id).unwrap();
    let sets: Vec<(&str, Value)> = vec![
        ("B", true.into()),
        ("Sb", Value::SByte(-5)),
        ("U8", Value::Byte(250)),
        ("I16", Value::Int16(-1234)),
        ("U16", Value::UInt16(65000)),
        ("I32", Value::Int32(-7)),
        ("U32", Value::UInt32(4_000_000_000)),
        ("I64", Value::Int64(-1 << 50)),
        ("U64", Value::UInt64(u64::MAX)),
        ("F", Value::Float(3.25)),
        ("D", Value::Double(-1e300)),
        ("S", Value::String(None)),
        ("Dt", Value::DateTime(1)),
        ("G", Value::Guid([7; 16])),
        ("Bs", Value::ByteString(Some(vec![1, 2, 3]))),
        ("Sc", Value::StatusCode(0x8000_0000)),
        ("E", "B".into()),
        ("Sh", Value::Int16(9)),
    ];
    for (p, val) in sets {
        d.set_str(&mut v, p, val).unwrap();
    }
    let bytes = d.encode(&v).unwrap();
    assert_eq!(
        bytes.len(),
        1 + 1 + 1 + 2 + 2 + 4 + 4 + 8 + 8 + 4 + 8 + 4 + 8 + 16 + 7 + 4 + 1 + 2
    );
    assert_eq!(d.decode(id, &bytes).unwrap(), v);
}

#[test]
fn unresolved_types_fail_lazily() {
    let d = TypeDictionary::parse(ALL_TYPES_XML.as_bytes()).unwrap();
    let id = d.require_struct("WithUnknown").unwrap();
    match d.decode(id, &[0; 32]).map_err(Error::into_kind) {
        Err(ErrorKind::UnresolvedType { owner, field, ty }) => {
            assert_eq!(
                (owner.as_str(), field.as_str(), ty.as_str()),
                ("WithUnknown", "Node", "NodeId")
            );
        }
        other => panic!("{other:?}"),
    }
    // Paths to the resolvable prefix still work on raw bytes.
    let p = d.compile_path(id, "Ok").unwrap();
    assert_eq!(
        d.read_path(&5i32.to_le_bytes(), &p).unwrap(),
        Value::Int32(5)
    );
}

#[test]
fn rejects_invalid_xml() {
    assert!(TypeDictionary::parse(b"<opc:StructuredType>").is_err());
    assert!(TypeDictionary::parse(b"<a><b></a>").is_err());
}

#[test]
fn bit_packed_types_are_isolated() {
    // Like the standard Opc.Ua dictionary: a 6-bit enum used in a bit-packed struct.
    let xml = r#"<opc:TypeDictionary xmlns:opc="http://opcfoundation.org/BinarySchema/" xmlns:tns="urn:t">
      <opc:EnumeratedType Name="NodeIdType" LengthInBits="6">
        <opc:EnumeratedValue Name="TwoByte" Value="0"/>
      </opc:EnumeratedType>
      <opc:StructuredType Name="Packed">
        <opc:Field Name="NodeIdType" TypeName="tns:NodeIdType"/>
        <opc:Field Name="Reserved" TypeName="opc:Bit" Length="2"/>
      </opc:StructuredType>
      <opc:StructuredType Name="Plain"><opc:Field Name="A" TypeName="opc:Int32"/></opc:StructuredType>
    </opc:TypeDictionary>"#;
    let d = TypeDictionary::parse(xml.as_bytes()).unwrap();
    let packed = d.require_struct("Packed").unwrap();
    assert!(matches!(
        d.decode(packed, &[0; 8]).map_err(Error::into_kind),
        Err(ErrorKind::Unsupported { .. })
    ));
    let plain = d.require_struct("Plain").unwrap();
    assert_eq!(
        d.decode(plain, &7i32.to_le_bytes()).unwrap().fields[0],
        Value::Int32(7)
    );
}
