type Cost = Vec<InventoryEntry>;

enum StructureType {
    WallLike,
    PillarLike,
    GroundFloorLike,
    CantileveFloorLike
}

struct Material {
    name: String,
    costs: BTreeMap<StructureType, Cost>
}