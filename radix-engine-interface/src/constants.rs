use crate::construct_address;
use crate::model::*;

// After changing Radix Engine ID allocation, you will most likely need to update the addresses below.
//
// To obtain the new addresses, uncomment the println code in `id_allocator.rs` and
// run `cd radix-engine && cargo test -- bootstrap_receipt_should_match_constants --nocapture`.
//
// We've arranged the addresses in the order they're created in the genesis transaction.

/// The address of the faucet package.
pub const FAUCET_PACKAGE: PackageAddress = construct_address!(
    EntityType::Package,
    141,
    155,
    15,
    11,
    75,
    56,
    26,
    144,
    11,
    25,
    176,
    220,
    194,
    134,
    211,
    7,
    178,
    76,
    145,
    105,
    44,
    106,
    6,
    99,
    122,
    49
);
pub const FAUCET_BLUEPRINT: &str = "Faucet";

/// The address of the account package.
pub const ACCOUNT_PACKAGE: PackageAddress = construct_address!(
    EntityType::Package,
    183,
    5,
    84,
    120,
    29,
    187,
    91,
    52,
    106,
    12,
    202,
    40,
    56,
    242,
    194,
    46,
    214,
    59,
    64,
    82,
    248,
    103,
    140,
    64,
    210,
    19
);
pub const ACCOUNT_BLUEPRINT: &str = "Account";

/// The ECDSA virtual resource address.
pub const ECDSA_SECP256K1_TOKEN: ResourceAddress = construct_address!(
    EntityType::Resource,
    197,
    145,
    49,
    208,
    59,
    205,
    57,
    14,
    91,
    255,
    113,
    67,
    162,
    242,
    190,
    254,
    113,
    134,
    95,
    83,
    154,
    232,
    216,
    228,
    190,
    35
);

/// The system token which allows access to system resources (e.g. setting epoch)
pub const SYSTEM_TOKEN: ResourceAddress = construct_address!(
    EntityType::Resource,
    116,
    117,
    173,
    206,
    105,
    144,
    92,
    116,
    248,
    225,
    130,
    72,
    94,
    142,
    60,
    167,
    52,
    186,
    5,
    29,
    146,
    198,
    120,
    157,
    206,
    226
);

/// The XRD resource address.
pub const RADIX_TOKEN: ResourceAddress = construct_address!(
    EntityType::Resource,
    199,
    24,
    137,
    61,
    178,
    84,
    252,
    213,
    183,
    107,
    209,
    173,
    144,
    5,
    46,
    12,
    223,
    13,
    133,
    8,
    176,
    152,
    95,
    216,
    120,
    51
);

/// The address of the faucet component, test network only.
pub const FAUCET_COMPONENT: ComponentAddress = construct_address!(
    EntityType::NormalComponent,
    139,
    102,
    112,
    90,
    86,
    241,
    123,
    106,
    194,
    118,
    77,
    122,
    228,
    192,
    200,
    254,
    97,
    228,
    48,
    125,
    233,
    170,
    107,
    105,
    87,
    105
);

pub const EPOCH_MANAGER: SystemAddress = construct_address!(
    EntityType::EpochManager,
    95,
    195,
    49,
    184,
    56,
    143,
    50,
    48,
    74,
    60,
    118,
    82,
    157,
    9,
    41,
    137,
    56,
    213,
    248,
    197,
    23,
    208,
    120,
    209,
    129,
    137
);

pub const CLOCK: SystemAddress = construct_address!(
    EntityType::Clock,
    100,
    122,
    90,
    153,
    192,
    230,
    68,
    232,
    52,
    111,
    194,
    67,
    139,
    246,
    24,
    111,
    166,
    139,
    122,
    227,
    235,
    71,
    163,
    178,
    99,
    94
);

/// The ED25519 virtual resource address.
pub const EDDSA_ED25519_TOKEN: ResourceAddress = construct_address!(
    EntityType::Resource,
    40,
    217,
    220,
    96,
    193,
    149,
    175,
    197,
    239,
    196,
    234,
    126,
    191,
    117,
    203,
    147,
    13,
    122,
    137,
    31,
    224,
    36,
    145,
    105,
    45,
    22
);

pub const EPOCH_MANAGER_BLUEPRINT: &str = "EpochManager";
pub const CLOCK_BLUEPRINT: &str = "Clock";
pub const RESOURCE_MANAGER_BLUEPRINT: &str = "ResourceManager";
pub const PACKAGE_BLUEPRINT: &str = "Package";
pub const TRANSACTION_PROCESSOR_BLUEPRINT: &str = "TransactionProcessor";
