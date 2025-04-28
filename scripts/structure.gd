extends Resource
class_name Structure

@export_subgroup("Model")
@export var model_file: String
var id: int # index into the MeshLibrary. Set at load time.
@export var tall_for_cutaway: bool

@export_subgroup("Gameplay")
@export var price: int # Price of the structure when building
@export var name: String
@export var placement_style: Globals.PlacementStyle
