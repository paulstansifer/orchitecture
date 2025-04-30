use core::panic;

use godot::prelude::*;

#[derive(Var, Export)]
#[godot(via=i64)]
enum PlacmentStyle {
    DoNotPlace,
    WallDrag,
    FloorDrag,
    RoomPlop,
    WallPlop,
}

impl GodotConvert for PlacmentStyle {
    type Via = i64;
}
impl ToGodot for PlacmentStyle {
    type ToVia<'v> = i64;
    fn to_godot(&self) -> i64 {
        match self {
            PlacmentStyle::DoNotPlace => 0,
            PlacmentStyle::WallDrag => 1,
            PlacmentStyle::FloorDrag => 2,
            PlacmentStyle::RoomPlop => 3,
            PlacmentStyle::WallPlop => 4,
        }
    }
}
impl FromGodot for PlacmentStyle {
    fn try_from_godot(value: i64) -> Result<Self, ConvertError> {
        match value {
            0 => Ok(PlacmentStyle::DoNotPlace),
            1 => Ok(PlacmentStyle::WallDrag),
            2 => Ok(PlacmentStyle::FloorDrag),
            3 => Ok(PlacmentStyle::RoomPlop),
            4 => Ok(PlacmentStyle::WallPlop),
            _ => panic!("bad enum"),
        }
    }
}

#[derive(GodotClass)]
#[class(base=Resource,no_init)]
struct Buildable {
    #[export]
    model_file: GString,
    #[export]
    id: i32,
    #[export]
    name: GString,
    #[export]
    placement_style: PlacmentStyle,
    base: Base<Resource>,
}
