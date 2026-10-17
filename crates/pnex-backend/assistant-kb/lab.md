---
id: lab
title: Vision Lab — teach PNeX your own objects
kind: feature
pages: /lab, /lab/:id
nodes: vision_detect
err_codes: camera-unknown, camera-export-empty, ml-model-load-failed, ml-model-inference-failed, lab-project-not-found, lab-image-not-found, lab-version-not-found, lab-class-in-use, lab-annotations-conflict, lab-box-invalid, lab-asset-not-image, lab-dataset-empty, lab-video-unsupported, lab-video-too-large, lab-video-decode-failed, lab-interpolate-no-keyframes, lab-train-busy, lab-train-base-invalid, lab-train-no-data, lab-train-interrupted, lab-train-failed, lab-propagate-no-previous, lab-review-full
tools: 
tags: train, entraîner, fine-tune, affiner, epochs, époques, video, vidéo, mp4, mov, interpolate, interpoler, interpolation, recording, enregistrement, video frames, images de vidéo, pre-annotation, pré-annotation, labeling, étiquetage, suggest, proposer, vision lab, lab, dataset, annotation, annoter, boîte, bounding box, custom model, modèle personnalisé, reconnaissance d'objets, object recognition, training, entraînement, coco, yolox
---
Vision Lab (Media › Vision Lab) builds the dataset of a custom object detector: name the classes ("red_valve", "safety_helmet"…), add pictures from the media library, draw a box around every object, then freeze a dataset version and download it in COCO format.

## What you can do
- **New project**: a name and the list of classes. Classes can be added or renamed later; a class that still has boxes cannot be removed.
- **Three steps on a line next to the title** (1 Pictures, 2 Label, 3 Train): **1 · Pictures** lists the sources of the project (each video, recording or batch of photos with how far it is labeled; **Label** opens it, **Remove** takes it out of the project with its labeling after a confirmation, the files staying in **Media**), **2 · Label** opens on the next picture to label (work resumes where it stopped), **3 · Train** opens the training window, also reached by the green **Train** button at the top right.
- **Add images** (step 1) asks where the pictures come from: **Photos of the media library**, **Videos of the media library** or **Recording of a camera**.
- **Photos**: pick pictures already uploaded in **Media**. Every import is one *source*: all its images go either to training or to validation, never split between both, so near-identical pictures never inflate the scores.
- **Recording of a camera**: take one picture every N seconds from what a camera recorded (pick the camera, the day, the start and end times; at most one hour and 200 pictures per import). The pictures land in the media library and in the project as one source, so a whole video stays on the same side of training and validation. Only device cameras with a **Video recording** flow have recordings.
- **Videos**: check one or several videos of the media library (MP4 or MOV in H.264, H.265 or MJPEG; at most 512 MB), a start, an end and an interval in seconds applied to each video (at most one hour and 200 pictures per video). Every import of the same video belongs to one source. Phone videos work as recorded. Another format is refused with "not a readable video": convert it to MP4 (H.264) first.
- **Timeline**: under the picture, the pictures of one source (pick it in the bar: each source shows how many of its pictures are labeled) as diamonds (yellow to label, green labeled, grey no object, violet to review), and one lane per class showing where it appears. Play steps through the pictures, the arrows and ← / → go to the previous / next one, a click on the timeline goes to the nearest picture. The **Classes** button of the timeline shows where each class appears.
- **Label**: pick a class with the round **+** at the top left of the picture (it lists the classes; keys 1 to 9 also pick one); its field adds a new class with Enter or the **+** button beside it, and the list stays open to add several in a row, drag a box around each object. Click a box to select it, **Delete** removes it. **Nothing to label here** marks a negative example (it teaches the model what is *not* the object). Every change is saved at once. The zoom at the top right of the picture (or the + / − keys) enlarges it; a zoomed picture scrolls inside its frame. While zoomed, the **hand** button (or the H key) turns a drag into a move of the picture instead of a new box, and the **whole picture** button (or the 0 key) zooms back out.
- **Pre-label**: pick a detection model of **Automation › Models** (for example the YOLOX COCO model) and **Suggest boxes**: the model proposes dashed boxes. A proposal whose label is a class name of the project (person, car, dog… or your own trained classes) takes that class; **Keep the matching proposals** keeps them all, a click keeps one (a grey proposal takes the current class), **Dismiss** drops the rest. Nothing is saved until you keep a box. Once a first custom model exists, use it here to annotate the next pictures faster.
- **Interpolate** (pictures from a video or a recording): label a picture, skip a few, label a later one, then on a picture in between click **Interpolate**: PNeX proposes the boxes moved in a straight line between the two labeled pictures of the same video, labelled "interpolated". Boxes are paired by class, nearest first; a box with no partner of its class on the other side is not proposed. Keep them like model proposals and correct the ones that drifted. It needs an labeled picture before and after in the same source.
- **Statistics**: boxes and images per class, train / validation counts, and warnings: fewer than 20 labeled images for a class, fewer than 5 validation images, no negative example. They are advice, never a blocker.
- **Train the model** (green **Train** button) trains on the current pictures: they are frozen into a new version on the way. **Advanced options** pick an older version, the starting model and the epochs.
- **Versions and export (COCO)** (training window, folded): **Freeze a version** copies the labeled images and their boxes into a numbered, read-only dataset version; later edits never change it.
- **Export**: downloads a version as a zip (images + COCO annotations for train and validation), ready for a YOLOX training.

## Improvement loop
- **To review**: in an **Object detection** node, **Doubtful frames to the Vision Lab** picks a project; when the model hesitates (a score between **Min score** and **Max score**, 0.25 to 0.6 by default), the frame joins that project with the model's proposals, at most one per camera every 60 s. In the Lab those pictures carry a purple dot ("To review"), **Only the pictures to review** filters them, and opening one shows the proposals ready to keep. A picture leaves the queue once it has boxes or is marked without object. At most 500 pictures wait per project.
- **Propagate** (pictures from a video or a recording, a model chosen in the proposals bar): the model proposes the boxes of the picture, and each takes the class of the nearest box on the previous labeled picture of the same video. Useful when the model does not know your class names.
- **Diversity warnings**: besides the counts, the Lab warns when a class comes from fewer than 3 sources, always has the same size (always at the same distance) or the same place in the picture, and when all pictures have the same brightness. Add the missing kind of pictures, then train again.

## Training
In the **Training** panel: pick a frozen **Dataset version**, a **Starting model** (a YOLOX model of **Automation › Models**, for example the YOLOX COCO model) and a number of **Epochs** (30 by default), then **Train**. PNeX keeps the starting network and learns again only its last layers for your classes, on the server, without GPU: a few minutes for a few hundred pictures. The run shows its progress (reading the pictures, epochs with the loss, evaluation) and, once done, the **validation mAP@0.5** (how well it finds the objects of the validation pictures, 100 % = all found with no false alarm). The new model joins **Automation › Models** with your class names, already checked: pick it in an **Object detection** node.

- One training per project at a time ("already queued or running"); **Cancel** stops it.
- No validation picture in the version = no mAP: give each class pictures from several imports or force some to validation.
- A run stopped by a server restart shows "the training stopped": start it again.
- Objects very different from everyday ones (the starting model knows people, vehicles, animals, objects of daily life) are learned less well with this method: add varied pictures, more epochs, and compare the mAP of the versions.
- The exported COCO zip stays available to train elsewhere with any YOLOX tool; the ONNX is registered in **Automation › Models** the same way.

## Good to know
- Label the object in many situations: angles, distances, light, partly hidden. Twenty varied pictures beat a hundred identical ones.
- A box should hug the visible part of the object, including when it touches the image border.
- Two people editing the same image at the same time: the second save is refused ("changed meanwhile"); reload the image and redo the change.
- Only Owners, Admins and Members change a project; Viewers can browse and export.
