# PCB Forge
PCB Forge is a tool for generating GCode files from gerber files and drill files. Its main intent is to manufacture Printed Circuit Boards using CNC machines such as milling machines and laser cutters.

This readme file will quickly cover how the config file formats work and what prototyping/manufacturing processes can be used.

# Disclaimer
This tool is currently an early work and not well tested. It is your responsibility to validate the generated GCode that it will produce the expected products and not damage your machine. If you discover that your input files produce gcode that is incorrect or may damage a machine, please open an issue providing all your input files and explain what is wrong with the generated gcode.

I use [ncviewer.com](https://ncviewer.com/) as a way to visualize GCode while testing. If your machine comes with a visualizer, you should prefer that as it's likely more accurate to your machine.

# Supported inputs and outputs.

## Inputs
PCB Forge can accept Gerber Files and Drill files.
An additional yaml file to specify the manufacturing process will be needed with these.

## Outputs
PCB Forge only outputs GCode files. Multiple GCode files can be produced from a single board to make switching between machines and tools easier.

# Prototyping techniques
Being able to produce PCBs with through holes and solder masks at home within a day can already be considered rapid prototyping by some standards, but you can test the measurements and generated gcode with the following techniques within minutes and extremely low risks.

## Laser Cut Cardboard
Using the laser, you can engrave and cut a PCB in less than 20 minutes out of cardboard. Although electrically useless, this is a great way to check the measurements of your footprints or to verify the design will fit on the copper plate you plan to use for the real thing. It's also a good way to get use to the workflow of PCB Forge.

## Pen Plotted Paper
A pen plotter uses similar movements to an end mill and a marker can have a similar diameter to one as well. This can be a good way to practice generating gcode for an end mill without the risk of breaking bits or wasting copper plates.

# Fabrication techniques

These are techniques you can use to actually fabricate real circuit boards with PCB Forge.

## Spray Painted etching mask

This process is derived from a [video by Robonza](https://www.youtube.com/watch?v=RuSg7-hMaQg&t=610s). Actually that video inspired this whole project. This technique should provide the best speed and precision, but is still a work in progress.

1. Get an FR4 board with a copper plating.
2. Use a CNC mill to cut through holes into your copper plate and then cut out the board shape.
3. Clean the board so that it is free of dust and finger prints. I like to spray it with compressed air and then rub it down with acetone or rubbing alcohol.
4. Spray paint the copper side. A primer meant for metals is ideal.
5. Secure a sacrificial piece of cardboard onto your laser cutter. Cut the board outline into it. Insert your PCB into the hole created. This jig should perfectly align your PCB with the coordinate system of your laser cutter.
6. Use the laser cutter to etch away paint on areas you wish to remove copper from.
7. Use an etchant to remove the copper. I use [a home made concoction](https://www.instructables.com/Is-the-best-PCB-etchant-in-every-kitchen-/) for my etchant. Please check with local authorities on how to dispose of bi-products responsibly. Heating the solution will make it go faster but be careful not to go too hot. I've found 75C to be pretty good.
8. Remove the board from the etchant and rinse with water. If you heated the board during the etching process, make sure to use hot water. Cold water will create thermal shock that causes the paint to delaminate.
9. Once dry, Spray paint on another layer and etch away the solder mask layer, giving you a solder mask.

## Milling

I haven't tried this technique myself yet but it should work. It looks right in the simulations and the machine worked correctly when a pen plotter was used to test the generated code.

You can use milling for the entire fabrication process of your board, but this is much less precise and very time consuming. You also won't be able to create solder masks with this technique.

# Config Files
There are two config files that PCB Forge reads from. The first is a global config used to provide default values to all your projects. This config file is entirely optional.

The second is the Forge File. This one is mandatory and every project must have at least one. It will specify how gcode files for your fabrication process should be generated.

All numeric values in config files have their units specified after them. This isn't just for looks, PCB Forge will convert your units into an internal format and then into whatever format your CNC machine supports. If the manual for your CNC machine specifies its bed size in inches, just type them into your config file as inches and PCB Forge will do the conversions for you.

# Global Config file
The global config file is optional. You can store machine and tool profiles to be globally accessible. You can also specify default profiles to use when none is specified in the forge file. If values are specified in the forge file, they will override what is specified in this file.

## Global Config Example
This is an example of a global config for an A350 Snap Maker. This example is not maintained and likely contains less than ideal configurations.
I will likely keep a maintained copy of my configurations elsewhere for you to use as a base once I dial in the manufacturing process more.
```yaml
# This is a list of machines you have available.
# Machines can also be specified in the forge file. Entries in the forge file will override entries in this file.
machines:
  # The name of the machine.
  snap_maker:
    # The max speed it can move at. This will be used for jog operations.
    jog_speed: 3000 mm/s

    # This is a lost of tool heads available to the machine.
    # You have the options of lasers or spindles.
    # The snap maker has interchangeable tool heads so it has both lasers and spindles.
    # If your machine has only one dedicated purpose, then only specify one spindle or one laser.
    tools:
      # The name for my laser.
      10w_laser:
        !laser
          # The diameter of the beam where it contacts the board.
          point_diameter: 0.2 mm

          # The maximum power this laser can output.
          max_power: 10 W

          # GCode that will be inserted near the beginning of generated gcode files
          # to initialize the tool. This particular one turns on the laser's fan.
          # The gcode file is expected to be in the same file as this list of machines.
          init_gcode: power_on_laser_fan.gcode

      # The name for my spindle.
      spindle:
        !spindle
          # The maximum speed of the spindle.
          # Even if the spindle doesn't support variable speed, you should
          # accurately specify this to insure gcode is generated correctly.
          max_speed: 120000 rpm

          # End mills that can be installed in the spindle.
          # Currently only end mills are supported, but support for
          # drill bits may be added in the future.
          bits:
            # A name for the end mill.
            square_end_mill:
              !end_mill
                # The diameter of the end mill.
                diameter: 0.5 mm

      # I 3D printed a pen plotter attachment for my snap maker.
      # We treat it as a spindle because it uses a similar movement.
      plotter:
        !spindle
          # The actual spindle needs to be plugged into the snap maker to
          # make it go into CNC mode. I don't want that to actually
          # spin, so we set its max RPM to zero so that gcode to start it
          # is never generated.
          max_speed: 0 rpm
          bits:
            # A common 0.3mm bic pen.
            bic_pen:
              !end_mill
                diameter: 0.3 mm

    # Configurations used to engrave the board.
    # This can be used for creating an etching mask or milling
    # traces directly into the board.
    engraving_configs:
      # A profile for making cardboard prototypes.
      cardboard_prototype:
        # Select the tool to be used.
        tool: 10w_laser

        # How fast to move while cutting.
        work_speed: 3000.0 mm/s

        # How much power to use while cutting. It is important
        # that you configured the maximum power correctly, otherwise
        # this may generate incorrect gcode.
        laser_power:  0.375 W

        # How many times to pass over the board while engraving.
        passes: 2

      # Engraves spray paint off copper to make an etching mask.
      copper_plate:
        tool: 10w_laser
        work_speed: 3000.0 mm/s
        laser_power:  0.75 W
        passes: 4

      # A "spindle" can also engrave.
      # You could use this to entirely mill a PCB, rather than etch it, but etching is much faster and more precise.
      # This pen "engraver" is a good way to see how an end mill would
      # be used for the engraving process.
      bic_pen:
        # The path selects the tool and then the bit.
        tool: plotter/bic_pen
        # The speed at which to spin the "spindle". A pen shouldn't spin
        # so this is set to zero.
        spindle_speed: 0 rpm

        # The height the tool should travel at. This should be above the board's surface.
        travel_height: 1.0 mm

        # The depth the tool should cut down to.
        # This can be done in multiple passes (see cutting configs below)
        # Zero represents the board's surface, so normally you'd want to go
        # below that, but because we're plotting with a pen, we want to
        # ride right on the surface.
        cut_depth: 0.0 mm

        # The speed at which to plunge the tool.
        plunge_speed: 3000.0 mm/s

        # The speed at which the tool can "cut" at.
        work_speed: 3000.0 mm/s

    # Configurations used for cutting the board.
    # This will be used for cutting through holes and the board's outline/shape.
    cutting_configs:
      # A laser can be used for cutting.
      cardboard_prototype:
        tool: 10w_laser
        work_speed: 1000.0 mm/s
        laser_power: 10 W
        passes: 1

      # Cut through the copper plate with an end mill.
      copper_plate:
        # The path selects the tool and then the bit.
        tool: spindle/square_end_mill

        # The speed at which to spin the tool at. On machines that do not
        # support adjusting the speed, just set this to the maximum speed.
        # Positive values will spin clockwise and negative values will spin counter clockwise.
        spindle_speed: 12000 rpm

        # The height the tool should travel at. This should be above the board's surface.
        travel_height: 1.0 mm

        # The depth the tool should cut down to.
        # This can be done in multiple passes (see below)
        cut_depth: -2.0 mm

        # Many end mills will break if you try to cut away too much
        # material at once. You can cut into the PCB in multiple passes.
        # This is the maximum depth a tool should cut at any given pass.
        pass_depth: 0.25 mm

        # The speed at which to plunge the tool.
        plunge_speed: 2.5 mm/s

        # The speed at which the tool can cut at.
        work_speed: 5 mm/s

      # Just another example but with a bic pen this time.
      # It doesn't actually cut.
      bic_pen:
        tool: plotter/bic_pen
        spindle_speed: 0 rpm
        travel_height: 1.0 mm
        cut_depth: 0.0 mm
        plunge_speed: 3000.0 mm/s
        work_speed: 3000.0 mm/s

    # The size of the machine's working area.
    workspace_area:
      width: 32.0 cm
      height: 34.0 cm

# This value is optional. Any stage in a forge file that doesn't specify 
# which engraving config to use will default to this one.
default_engraver: snap_maker/cardboard_prototype

# This value is optional. Any stage in a forge file that doesn't specify 
# which cutting config to use will default to this one.
default_cutter: snap_maker/cardboard_prototype
```

# Forge File
A forge file specifies the gcode files to be generated and how they are to be generated. The configuration you use will depend on your project and the fabrication process you chose to use.

Note that the `machines` section from the global config can be specified in the forge file as well. Machines defined here will override what is in the global config. This is ideal for quick onboarding with teams.

The following is an example from a board I made for some home made smart blinds.
```yaml
# Meta data.
# Currently not used but will eventually be included in gcode files as metadata.
project_name: "Window Blind Motor"
board_version: 1.0.0

# GCode files will be generated in an arbitrary order.
gcode_files:
  # The gcode file to be generated.
  drill.gcode:
    # We now list the stages to be generated.
    # Each will be generated and added to the gcode file in the order they are defined here.
    - !cut_board # Cut through holes.
        # You can specify a drill hole file or a gerber file for cutting.
        drill_file: WindowBlindMotor-PTH.drl

        # The gcode file to append the generated code to.
        gcode_file: drill.gcode

        # The machine configuration to use when generating the gcode.
        machine_config: snap_maker/copper_plate

        # If set to true, the generated gcode will be inverted on the X
        # axis, perfect for cutting or engraving from the back side of
        # the board. Defaults to false.
        backside: true
    - !cut_board # Cut board outline.
        # You can also use gerber files.
        gerber_file: WindowBlindMotor-Edge_Cuts.gbr
        machine_config: snap_maker/copper_plate

        # You can set this to inner, outer, or all.
        # All will generate gcode to cut out the exact shape in the gerber file.
        # inner will only cut out the inner holes of the gerber file.
        # outer will cause it to only cut out the outline, or the figure of the shape.
        select_lines: outer
        backside: true
  jig.gcode:
    - !cut_board # Cut a jig to make sure we align the board right.
        gerber_file: WindowBlindMotor-Edge_Cuts.gbr
        machine_config: snap_maker/cardboard_prototype
        select_lines: outer
  etching.gcode:
    - !engrave_mask # Engrave back copper etching mask.
        gerber_file: WindowBlindMotor-B_Cu.gbr
        machine_config: snap_maker/copper_plate
        backside: true
  silkscreen.gcode:
    - !engrave_mask # Engrave silkscreen
        gerber_file: WindowBlindMotor-F_Silkscreen.gbr
        machine_config: snap_maker/copper_plate
        invert: true
  solder_mask.gcode:
    - !engrave_mask # Engrave solder mask.
        gerber_file: WindowBlindMotor-B_Mask.gbr
        machine_config: snap_maker/copper_plate
        backside: true
        invert: true
```