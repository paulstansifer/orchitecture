extends Resource
class_name Structure

@export_subgroup("Model")
@export var model_file: String
var model: PackedScene # Model of the structure
var id: int # index into the MeshLibrary. Set at load time.

@export_subgroup("Gameplay")
@export var price: int # Price of the structure when building
@export var name: String
@export var placement_style: Globals.PlacementStyle
