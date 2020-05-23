use crate::validate_uuid;

#[test]
pub fn test_val_uuid() {
    assert!(validate_uuid("00001104-0000-1000-8000-00805f9b34fb"));
    assert!(validate_uuid("8a33385f-4465-47aa-a25a-3631f01d4861"));
    assert!(validate_uuid(
        &"8a33385f-4465-47aa-a25a-3631f01d4861".to_uppercase()
    ));
    assert!(!validate_uuid("00001104-0000-1000-8000-00805f9b34f")); // too short
    assert!(!validate_uuid("00001104-0000-1000-8000-00805f9b34fb ")); // too long
    assert!(!validate_uuid("8a33385f.4465.47aa.a25a.3631f01d4861")); // wrong delimiter
    assert!(!validate_uuid("8h33385h-4465-47hh-a25h-3631fh1h4861")); // not hex
    assert!(!validate_uuid("-h33385h-4465-47hh-a25h-3631fh1h4861")); // first number is negative
}
